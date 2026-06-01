//! SimpleX Chat gateway — spawns simplex-chat CLI as a subprocess,
//! reads JSON output line by line, and sends replies via simplex-chat send.
//!
//! Environment variables:
//!   SIMPLEX_CLI_PATH — path to the simplex-chat CLI binary (default: "simplex-chat")
//!
//! Reference: https://github.com/simplex-chat/simplex-chat/blob/stable/docs/CLI.md

use arli_core::{
    Agent, AgentConfig, AgentMessage, Config,
    OpenAIProvider, SessionStore, ToolRegistry,
    memory::MemoryStore,
};
use arli_core::tools::builtin::register_builtin_tools;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::Mutex;
use tracing::{info, warn, error};

// ── SimpleX JSON output types ──

/// Incoming message event from simplex-chat stdout.
#[derive(Debug, Deserialize)]
struct SimplexEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    #[serde(rename = "msgId")]
    msg_id: Option<i64>,
    #[serde(rename = "chatMsgEvent")]
    chat_msg_event: Option<String>, // "rcv message"
    text: Option<String>,
    #[serde(rename = "contactId")]
    contact_id: Option<i64>,
    #[serde(rename = "remoteDisplayName")]
    remote_display_name: Option<String>,
    #[serde(rename = "connId")]
    conn_id: Option<i64>,
}

struct SimplexGateway {
    cli_path: String,
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl SimplexGateway {
    fn new(cli_path: String, data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            cli_path,
            agents: Mutex::new(HashMap::new()),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

    /// Send a reply via `simplex-chat send`
    async fn send_message(cli_path: &str, contact_id: &str, text: &str) -> anyhow::Result<()> {
        let truncated = if text.len() > 4000 {
            format!("{}... _(truncated)_", &text[..4000])
        } else {
            text.to_string()
        };

        let output = Command::new(cli_path)
            .args(["-c", &format!("/_send {} {}", contact_id, truncated)])
            .output()
            .await?;

        if !output.status.success() {
            warn!(
                "simplex-chat send failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &PathBuf,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        contact_id: &str,
        cli_path: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        if let Some(sender) = agents.get(contact_id) {
            return Ok(sender.clone());
        }

        let safe_id = contact_id.replace(['/', ' ', ':', '@', '\\'], "_");
        info!("Creating SimpleX agent for contact {}", contact_id);

        let db_path = data_dir.join(format!("simplex-{}.db", safe_id));
        let store = SessionStore::open(db_path.clone())?;
        let memory_path = data_dir.join("memory.db");
        let memory_store = Arc::new(MemoryStore::open(memory_path)?);

        let provider = Box::new(OpenAIProvider::new(
            provider_api_key.to_string(),
            model.to_string(),
            provider_base_url.clone(),
        ));

        let mut tools = ToolRegistry::new();
        register_builtin_tools(&mut tools, Some(db_path), Some(memory_store), None, None);

        let agent_config = AgentConfig {
            name: format!("simplex-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via SimpleX Chat. \
                 Respond in the user's language. Be concise.".to_string()
            ),
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
        let contact_owned = contact_id.to_string();
        let cli_owned = cli_path.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        let truncated = if response.len() > 4000 {
                            format!("{}... _(truncated)_", &response[..4000])
                        } else {
                            response
                        };
                        if let Err(e) = Self::send_message(
                            &cli_owned,
                            &contact_owned,
                            &truncated,
                        ).await
                        {
                            error!("Failed to send SimpleX response: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("SimpleX agent error for contact {}: {}", contact_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(contact_id.to_string(), sender.clone());
        Ok(sender)
    }

    async fn run_forever(self: Arc<Self>) -> anyhow::Result<()> {
        info!("SimpleX gateway starting, CLI: {}", self.cli_path);

        let mut child = Command::new(&self.cli_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let stdin: ChildStdin = child.stdin.take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture simplex-chat stdin"))?;
        let stdin_writer = Arc::new(Mutex::new(stdin));

        let stdout = child.stdout.take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture simplex-chat stdout"))?;
        let reader = BufReader::new(stdout);

        let mut lines = reader.lines();

        info!("SimpleX CLI started, reading output...");

        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            // Parse JSON event
            let event: SimplexEvent = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue, // Skip non-JSON lines
            };

            // Only process incoming messages
            let chat_event = event.chat_msg_event.as_deref().unwrap_or("");
            if !(chat_event == "rcv message" || chat_event == "rcv group message"
                || event.event_type.as_deref() == Some("rcvMessage"))
            {
                continue;
            }

            let text = match &event.text {
                Some(t) => t.trim().to_string(),
                None => continue,
            };

            if text.is_empty() {
                continue;
            }

            let contact_id = event
                .contact_id
                .map(|id| id.to_string())
                .or_else(|| event.conn_id.map(|id| id.to_string()))
                .unwrap_or_else(|| "unknown".to_string());

            info!("SimpleX message from {}: {}", contact_id, text);

            match SimplexGateway::get_or_create_agent(
                &self.agents,
                &self.data_dir,
                &self.provider_api_key,
                &self.provider_base_url,
                &self.model,
                &contact_id,
                &self.cli_path,
            ).await
            {
                Ok(sender) => {
                    if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                        error!("Failed to send to SimpleX agent {}: {}", contact_id, e);
                        self.agents.lock().await.remove(&contact_id);
                    }
                }
                Err(e) => {
                    error!("Cannot create SimpleX agent for {}: {}", contact_id, e);
                }
            }
        }

        // Process ended — try to reap
        let _ = child.wait().await;
        let _ = stdin_writer; // keep alive

        Ok(())
    }
}

pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let cli_path = std::env::var("SIMPLEX_CLI_PATH")
        .unwrap_or_else(|_| "simplex-chat".to_string());

    let gateway = Arc::new(SimplexGateway::new(cli_path, data_dir)?);
    gateway.run_forever().await
}
