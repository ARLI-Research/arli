//! ntfy gateway — polls ntfy.sh for new messages on a topic,
//! replies by publishing to the same topic.
//!
//! Environment variables:
//!   NTFY_TOPIC  — topic to subscribe to
//!   NTFY_SERVER — ntfy server base URL (default: https://ntfy.sh)
//!
//! Reference: https://docs.ntfy.sh/

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

// ── ntfy JSON types ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct NtfyMessage {
    id: Option<String>,
    time: Option<i64>,
    message: Option<String>,
    title: Option<String>,
    topic: Option<String>,
}

struct NtfyGateway {
    server: String,
    topic: String,
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    last_id: Mutex<Option<String>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl NtfyGateway {
    fn new(server: String, topic: String, data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            server: server.trim_end_matches('/').to_string(),
            topic,
            agents: Mutex::new(HashMap::new()),
            last_id: Mutex::new(None),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

    /// Poll ntfy for new messages. Uses `since=` param for incremental polling.
    async fn poll(&self) -> anyhow::Result<Vec<NtfyMessage>> {
        let url = format!("{}/{}/json", self.server, self.topic);
        let client = reqwest::Client::new();
        let mut req = client.get(&url);

        let last_id = self.last_id.lock().await;
        if let Some(ref since) = *last_id {
            req = req.query(&[("since", since.as_str())]);
        }
        drop(last_id);

        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            warn!(
                "ntfy poll returned {}: {}",
                status,
                resp.text().await.unwrap_or_default()
            );
            return Ok(vec![]);
        }

        let messages: Vec<NtfyMessage> = resp.json().await?;

        // Track last seen id
        if let Some(last_msg) = messages.last() {
            if let Some(ref id) = last_msg.id {
                *self.last_id.lock().await = Some(id.clone());
            }
        }

        Ok(messages)
    }

    /// Publish a reply to the ntfy topic.
    async fn send_message(&self, text: &str) -> anyhow::Result<()> {
        let url = &self.server;
        let client = reqwest::Client::new();
        let truncated = if text.len() > 4000 {
            format!("{}... _(truncated)_", &text[..4000])
        } else {
            text.to_string()
        };
        client
            .post(url)
            .body(truncated)
            .header("Title", "ARLI Response")
            .send()
            .await?;
        Ok(())
    }

    async fn get_or_create_agent(&self) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let chat_key = &self.topic;
        let mut agents = self.agents.lock().await;

        if let Some(sender) = agents.get(chat_key) {
            return Ok(sender.clone());
        }

        info!("Creating ntfy agent for topic {}", chat_key);

        let safe_id = chat_key.replace(['/', ' ', ':', '@', '\\'], "_");
        let db_path = self.data_dir.join(format!("ntfy-{}.db", safe_id));
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
        );

        let agent_config = AgentConfig {
            name: format!("ntfy-{}", safe_id),
            session_id: None,
            system_prompt: Some(format!(
                "You are ARLI, an AI agent communicating via ntfy. \
                 Current topic: {}. Respond in the user's language. Be concise.",
                chat_key
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
        let server = self.server.clone();
        let topic = self.topic.clone();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        let truncated = if response.len() > 4000 {
                            format!("{}... _(truncated)_", &response[..4000])
                        } else {
                            response
                        };
                        let url = format!("{}/{}", server, topic);
                        if let Err(e) = reqwest::Client::new()
                            .post(&url)
                            .body(truncated)
                            .header("Title", "ARLI Response")
                            .send()
                            .await
                        {
                            error!("Failed to send ntfy response: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("ntfy agent error for topic {}: {}", topic, e);
                        break;
                    }
                }
            }
        });

        agents.insert(chat_key.to_string(), sender.clone());
        Ok(sender)
    }

    async fn process_messages(&self, messages: Vec<NtfyMessage>) {
        for msg in messages {
            let text = match &msg.message {
                Some(t) => t.trim().to_string(),
                None => continue,
            };

            if text.is_empty() {
                continue;
            }

            info!("ntfy message on {}: {}", self.topic, text);

            match self.get_or_create_agent().await {
                Ok(sender) => {
                    if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                        error!("Failed to send to ntfy agent: {}", e);
                        self.agents.lock().await.remove(&self.topic);
                    }
                }
                Err(e) => {
                    error!("Cannot create ntfy agent: {}", e);
                }
            }
        }
    }

    async fn run_forever(self: Arc<Self>) {
        info!("ntfy gateway starting for topic: {}", self.topic);
        loop {
            match self.poll().await {
                Ok(messages) => {
                    if !messages.is_empty() {
                        self.process_messages(messages).await;
                    }
                }
                Err(e) => {
                    error!("ntfy poll error: {}", e);
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        }
    }
}

pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let topic = std::env::var("NTFY_TOPIC")
        .ok()
        .ok_or_else(|| anyhow::anyhow!("NTFY_TOPIC not set. Set NTFY_TOPIC env var."))?;

    let server = std::env::var("NTFY_SERVER").unwrap_or_else(|_| "https://ntfy.sh".to_string());

    let gateway = Arc::new(NtfyGateway::new(server, topic, data_dir)?);
    gateway.run_forever().await;
    Ok(())
}
