//! Signal gateway — polls `signal-cli` for new messages and replies via subprocess.
//!
//! Environment variables:
//!   SIGNAL_CLI_PATH        — Path to the signal-cli binary (default: "signal-cli")
//!   SIGNAL_PHONE_NUMBER    — Registered Signal phone number (e.g., "+1234567890")

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

// ── Signal JSON types ──

#[derive(Debug, serde::Deserialize)]
struct SignalEnvelope {
    source: Option<String>,
    #[serde(rename = "dataMessage")]
    data_message: Option<SignalDataMessage>,
    #[serde(rename = "syncMessage")]
    #[allow(dead_code)]
    sync_message: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize)]
struct SignalDataMessage {
    message: Option<String>,
    #[allow(dead_code)]
    timestamp: Option<i64>,
}

struct SignalGateway {
    cli_path: String,
    phone_number: String,
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl SignalGateway {
    fn new(cli_path: String, phone_number: String, data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            cli_path,
            phone_number,
            agents: Mutex::new(HashMap::new()),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

    async fn send_message(&self, recipient: &str, text: &str) -> anyhow::Result<()> {
        let output = tokio::process::Command::new(&self.cli_path)
            .args(["-u", &self.phone_number, "send", "-m", text, recipient])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("signal-cli send failed for {}: {}", recipient, stderr);
        }
        Ok(())
    }

    async fn receive_messages(&self) -> anyhow::Result<Vec<(String, String)>> {
        let output = tokio::process::Command::new(&self.cli_path)
            .args([
                "-u",
                &self.phone_number,
                "receive",
                "--json",
                "--ignore-attachments",
                "--timeout",
                "5",
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                warn!("signal-cli receive stderr: {}", stderr.trim());
            }
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut messages = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<SignalEnvelope>(line) {
                Ok(envelope) => {
                    let source = match envelope.source {
                        Some(ref s) => s.clone(),
                        None => continue,
                    };
                    let text = match envelope.data_message {
                        Some(ref dm) => match dm.message {
                            Some(ref m) if !m.trim().is_empty() => m.clone(),
                            _ => continue,
                        },
                        None => continue,
                    };
                    messages.push((source, text));
                }
                Err(e) => {
                    warn!("Failed to parse signal-cli JSON line: {} — {:?}", e, line);
                }
            }
        }

        Ok(messages)
    }

    async fn get_or_create_agent(
        &self,
        sender_number: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = self.agents.lock().await;

        if let Some(sender) = agents.get(sender_number) {
            return Ok(sender.clone());
        }

        info!("Creating Signal agent for {}", sender_number);

        let safe_id = sender_number.replace('+', "");
        let db_path = self.data_dir.join(format!("signal-{}.db", safe_id));
        let store = SessionStore::open(db_path.clone())?;
        let memory_path = self.data_dir.join("memory.db");
        let memory_store = Arc::new(MemoryStore::open(memory_path)?);

        let provider = Box::new(OpenAIProvider::new(
            self.provider_api_key.clone(),
            self.model.clone(),
            self.provider_base_url.clone(),
        ));

        let mut tools = ToolRegistry::new();
        register_builtin_tools(
            &mut tools,
            Some(db_path),
            Some(memory_store),
            None,
            None,
            None,
            None,
        );

        let agent_config = AgentConfig {
            brokering: None,
            tenant_id: None,
            provider_name: None,
            name: format!("signal-{}", safe_id),
            session_id: None,
            system_prompt: Some(format!(
                "You are ARLI, an AI agent communicating via Signal. \
                 Respond in the user's language. Be concise — Signal messages work best short."
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
        let cli_path = self.cli_path.clone();
        let phone_number = self.phone_number.clone();
        let recipient = sender_number.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        let truncated = if response.len() > 4000 {
                            format!("{}... _(truncated)_", &response[..4000])
                        } else {
                            response
                        };
                        let output = tokio::process::Command::new(&cli_path)
                            .args(["-u", &phone_number, "send", "-m", &truncated, &recipient])
                            .output()
                            .await;
                        if let Err(e) = output {
                            error!("Failed to send Signal response: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Signal agent error for {}: {}", recipient, e);
                        break;
                    }
                }
            }
        });

        agents.insert(sender_number.to_string(), sender.clone());
        Ok(sender)
    }
}

/// Run the Signal gateway (poll loop).
pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let cli_path = std::env::var("SIGNAL_CLI_PATH").unwrap_or_else(|_| "signal-cli".to_string());
    let phone_number = std::env::var("SIGNAL_PHONE_NUMBER")
        .map_err(|_| anyhow::anyhow!("SIGNAL_PHONE_NUMBER not set"))?;

    info!(
        "Signal gateway starting (phone: {}, cli: {})...",
        phone_number, cli_path
    );

    let gateway = Arc::new(SignalGateway::new(cli_path, phone_number, data_dir)?);

    loop {
        match gateway.receive_messages().await {
            Ok(messages) => {
                for (source, text) in messages {
                    info!("Signal message from {}: {}", source, text);

                    match gateway.get_or_create_agent(&source).await {
                        Ok(sender) => {
                            if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                                error!("Failed to send to Signal agent {}: {}", source, e);
                                gateway.agents.lock().await.remove(&source);
                            }
                        }
                        Err(e) => {
                            error!("Cannot create Signal agent for {}: {}", source, e);
                        }
                    }
                }
            }
            Err(e) => {
                error!("Signal receive error: {}", e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}
