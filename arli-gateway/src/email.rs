//! Email gateway — polls IMAP for new messages and sends responses via SMTP.
//!
//! Each sender email address gets its own ARLI Agent with session persistence.
//! Agent responses are forwarded back as email replies.
//!
//! Environment variables:
//!   EMAIL_IMAP_SERVER  — IMAP server hostname (required)
//!   EMAIL_IMAP_PORT    — IMAP port (default: 993)
//!   EMAIL_SMTP_SERVER  — SMTP server hostname (required)
//!   EMAIL_SMTP_PORT    — SMTP port (default: 587)
//!   EMAIL_USER         — Login username / email address (required)
//!   EMAIL_PASSWORD     — Login password (required)

use arli_core::{
    Agent, AgentConfig, AgentMessage, Config,
    OpenAIProvider, SessionStore, ToolRegistry,
    memory::MemoryStore,
};
use arli_core::tools::builtin::register_builtin_tools;
use futures_util::TryStreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn, error};
use lettre::AsyncTransport;

// ── Email gateway state ──

struct EmailGateway {
    imap_server: String,
    imap_port: u16,
    smtp_server: String,
    smtp_port: u16,
    email_user: String,
    email_password: String,
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl EmailGateway {
    fn new(data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            imap_server: std::env::var("EMAIL_IMAP_SERVER")
                .map_err(|_| anyhow::anyhow!("EMAIL_IMAP_SERVER not set"))?,
            imap_port: std::env::var("EMAIL_IMAP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(993),
            smtp_server: std::env::var("EMAIL_SMTP_SERVER")
                .map_err(|_| anyhow::anyhow!("EMAIL_SMTP_SERVER not set"))?,
            smtp_port: std::env::var("EMAIL_SMTP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(587),
            email_user: std::env::var("EMAIL_USER")
                .map_err(|_| anyhow::anyhow!("EMAIL_USER not set"))?,
            email_password: std::env::var("EMAIL_PASSWORD")
                .map_err(|_| anyhow::anyhow!("EMAIL_PASSWORD not set"))?,
            agents: Mutex::new(HashMap::new()),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

    /// Connect to IMAP via TLS, login, select INBOX, and fetch unseen messages.
    /// Returns a list of (sender_email, subject, body, uid).
    async fn fetch_new_emails(&self) -> anyhow::Result<Vec<(String, String, String, u32)>> {
        let tls = async_native_tls::TlsConnector::new();
        let stream = async_std::net::TcpStream::connect((self.imap_server.as_str(), self.imap_port))
            .await
            .map_err(|e| anyhow::anyhow!("IMAP TCP connect failed: {}", e))?;
        let tls_stream = tls
            .connect(self.imap_server.as_str(), stream)
            .await
            .map_err(|e| anyhow::anyhow!("IMAP TLS handshake failed: {}", e))?;
        let client = async_imap::Client::new(tls_stream);

        let mut session = client
            .login(self.email_user.as_str(), self.email_password.as_str())
            .await
            .map_err(|(e, _)| anyhow::anyhow!("IMAP login failed: {}", e))?;

        session
            .select("INBOX")
            .await
            .map_err(|e| anyhow::anyhow!("IMAP select INBOX failed: {}", e))?;

        // Search for UNSEEN messages using UIDs so we can mark them seen later
        let uids = session
            .uid_search("UNSEEN")
            .await
            .map_err(|e| anyhow::anyhow!("IMAP search failed: {}", e))?;

        if uids.is_empty() {
            session
                .logout()
                .await
                .map_err(|e| anyhow::anyhow!("IMAP logout failed: {}", e))?;
            return Ok(vec![]);
        }

        let uid_list = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");

        // Fetch envelope and body text for all unseen messages
        let fetches: Vec<async_imap::types::Fetch> = session
            .uid_fetch(uid_list, "(ENVELOPE BODY[TEXT])")
            .await
            .map_err(|e| anyhow::anyhow!("IMAP fetch failed: {}", e))?
            .try_collect()
            .await
            .map_err(|e| anyhow::anyhow!("IMAP fetch stream collect failed: {}", e))?;

        let mut results = Vec::new();

        for fetch in fetches.iter() {
            let uid = match fetch.uid {
                Some(uid) => uid,
                None => continue,
            };

            let envelope = match fetch.envelope() {
                Some(e) => e,
                None => continue,
            };

            // Extract sender email address
            let from_addr = envelope
                .from
                .as_ref()
                .and_then(|addrs| addrs.first())
                .map(|addr| {
                    let mailbox = addr.mailbox.as_ref()
                        .and_then(|m| std::str::from_utf8(m).ok())
                        .unwrap_or("");
                    let host = addr.host.as_ref()
                        .and_then(|h| std::str::from_utf8(h).ok())
                        .unwrap_or("");
                    if host.is_empty() {
                        mailbox.to_string()
                    } else {
                        format!("{}@{}", mailbox, host)
                    }
                })
                .unwrap_or_else(|| "unknown".to_string());

            // Extract subject
            let subject = envelope
                .subject
                .as_ref()
                .map(|s| String::from_utf8_lossy(s).to_string())
                .unwrap_or_else(|| "(no subject)".to_string());

            // Extract body text
            let body = fetch
                .text()
                .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                .unwrap_or_else(|| "(no body)".to_string());

            results.push((from_addr, subject, body, uid));
        }

        // Mark fetched messages as seen
        let seen_uids = results.iter().map(|(_, _, _, uid)| uid.to_string()).collect::<Vec<_>>().join(",");
        if !seen_uids.is_empty() {
            if let Err(e) = session.uid_store(seen_uids, "+FLAGS (\\Seen)").await {
                warn!("Failed to mark emails as seen: {}", e);
            }
        }

        session
            .logout()
            .await
            .map_err(|e| anyhow::anyhow!("IMAP logout failed: {}", e))?;

        Ok(results)
    }

    /// Send an email reply via SMTP.
    #[allow(dead_code)]
    async fn send_email_reply(
        &self,
        to: &str,
        subject: &str,
        body: &str,
    ) -> anyhow::Result<()> {
        let creds = lettre::transport::smtp::authentication::Credentials::new(
            self.email_user.clone(),
            self.email_password.clone(),
        );

        let mailer = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous(
            self.smtp_server.as_str(),
        )
        .port(self.smtp_port)
        .credentials(creds)
        .build();

        let reply_subject = if subject.starts_with("Re:") {
            subject.to_string()
        } else {
            format!("Re: {}", subject)
        };

        let email = lettre::Message::builder()
            .from(
                format!("ARLI <{}>", self.email_user)
                    .parse()
                    .map_err(|e| anyhow::anyhow!("Invalid from address: {}", e))?,
            )
            .to(to
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid to address: {}", e))?)
            .subject(reply_subject)
            .body(body.to_string())
            .map_err(|e| anyhow::anyhow!("Failed to build email: {}", e))?;

        mailer
            .send(email)
            .await
            .map_err(|e| anyhow::anyhow!("SMTP send failed: {}", e))?;

        Ok(())
    }

    /// Get or create an agent for a sender email address.
    async fn get_or_create_agent(
        &self,
        sender_email: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = self.agents.lock().await;

        if let Some(sender) = agents.get(sender_email) {
            return Ok(sender.clone());
        }

        info!("Creating Email agent for {}", sender_email);

        // Sanitize email for use as a filename
        let safe_key = sender_email.replace(['@', '.'], "-");
        let db_path = self.data_dir.join(format!("email-{}.db", safe_key));
        let store = SessionStore::open(db_path.clone())?;
        let memory_path = self.data_dir.join("memory.db");
        let memory_store = Arc::new(MemoryStore::open(memory_path)?);

        let provider = Box::new(OpenAIProvider::new(
            self.provider_api_key.clone(),
            self.model.clone(),
            self.provider_base_url.clone(),
        ));

        let mut tools = ToolRegistry::new();
        register_builtin_tools(&mut tools, Some(db_path), Some(memory_store), None, None);

        let agent_config = AgentConfig {
            name: format!("email-{}", safe_key),
            session_id: None,
            system_prompt: Some(format!(
                "You are ARLI, an AI agent communicating via email with {}. \
                 Respond in the user's language. Be concise and professional.",
                sender_email
            )),
            protect_last_n: 20,
            protect_first_n: 3,
            token_budget: None,
            time_budget_secs: None,
            dollar_budget_cents: None,
        };

        let mut agent = Agent::new(
            agent_config,
            provider,
            tools,
            arli_core::PolicyEngine::default(),
            Some(store),
            20,
            None,
        );

        let sender = agent.sender();
        let sender_email_owned = sender_email.to_string();
        let smtp_server = self.smtp_server.clone();
        let smtp_port = self.smtp_port;
        let email_user = self.email_user.clone();
        let email_password = self.email_password.clone();

        // Spawn per-sender agent loop
        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        let body = if response.len() > 4000 {
                            format!("{}...\n\n_(truncated)_", &response[..4000])
                        } else {
                            response
                        };

                        let creds = lettre::transport::smtp::authentication::Credentials::new(
                            email_user.clone(),
                            email_password.clone(),
                        );

                        let mailer = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous(
                            smtp_server.as_str(),
                        )
                        .port(smtp_port)
                        .credentials(creds)
                        .build();

                        let email = match lettre::Message::builder()
                            .from(
                                format!("ARLI <{}>", email_user)
                                    .parse()
                                    .unwrap_or_else(|_| {
                                        format!("{}", email_user).parse().unwrap()
                                    }),
                            )
                            .to(match sender_email_owned.parse() {
                                Ok(a) => a,
                                Err(e) => {
                                    error!("Invalid recipient {}: {}", sender_email_owned, e);
                                    break;
                                }
                            })
                            .subject("Re: ARLI Response")
                            .body(body)
                        {
                            Ok(m) => m,
                            Err(e) => {
                                error!("Failed to build email for {}: {}", sender_email_owned, e);
                                break;
                            }
                        };

                        if let Err(e) = mailer.send(email).await {
                            error!("SMTP send to {} failed: {}", sender_email_owned, e);
                        }
                    }
                    Err(e) => {
                        error!("Email agent error for {}: {}", sender_email_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(sender_email.to_string(), sender.clone());
        Ok(sender)
    }

    /// Main loop: poll IMAP every 60 seconds and route new emails to agents.
    async fn run_forever(self: Arc<Self>) {
        info!(
            "Email gateway starting (IMAP: {}:{}, SMTP: {}:{}, user: {})",
            self.imap_server, self.imap_port,
            self.smtp_server, self.smtp_port,
            self.email_user,
        );

        loop {
            match self.fetch_new_emails().await {
                Ok(emails) => {
                    for (sender_email, subject, body, _uid) in emails {
                        info!(
                            "Email from {}: Subject: \"{}\"",
                            sender_email, subject
                        );

                        // Combine subject and body as the message content
                        let text = format!("Subject: {}\n\n{}", subject, body);

                        match self.get_or_create_agent(&sender_email).await {
                            Ok(agent_tx) => {
                                if let Err(e) = agent_tx.send(AgentMessage::UserMessage(text)).await {
                                    error!(
                                        "Failed to send to email agent {}: {}",
                                        sender_email, e
                                    );
                                    self.agents.lock().await.remove(&sender_email);
                                }
                            }
                            Err(e) => {
                                error!(
                                    "Cannot create email agent for {}: {}",
                                    sender_email, e
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Email poll error: {}", e);
                    // Wait a bit extra on error before retrying
                    tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        }
    }
}

/// Resolve email config from environment variables.
fn resolve_email_config() -> Option<(String, String)> {
    let user = std::env::var("EMAIL_USER").ok()?;
    let password = std::env::var("EMAIL_PASSWORD").ok()?;
    let imap = std::env::var("EMAIL_IMAP_SERVER").ok()?;
    let smtp = std::env::var("EMAIL_SMTP_SERVER").ok()?;
    Some((format!("{}|{}|{}|{}", imap, smtp, user, password), user))
}

/// Run the Email gateway.
pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let _ = resolve_email_config().ok_or_else(|| {
        anyhow::anyhow!(
            "Email credentials not configured. Set EMAIL_IMAP_SERVER, EMAIL_SMTP_SERVER, \
             EMAIL_USER, and EMAIL_PASSWORD environment variables."
        )
    })?;

    let gateway = Arc::new(EmailGateway::new(data_dir)?);
    gateway.run_forever().await;
    Ok(())
}
