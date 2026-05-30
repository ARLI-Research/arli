//! ARLI Gateway — Telegram messaging bridge.
//!
//! Architecture:
//! - Long-polls Telegram for new messages
//! - Routes each message to a session-scoped Agent
//! - Posts agent responses back to the chat
//!
//! Uses raw Telegram HTTP API (no heavy framework) — minimal deps.

use arli_core::{
    Agent, AgentConfig, AgentMessage, Config,
    OpenAIProvider, PolicyEngine, SessionStore, ToolRegistry,
    memory::MemoryStore,
};
use arli_core::tools::builtin::register_builtin_tools;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn, error};

// ── Telegram API types ──

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    message_id: i64,
    chat: TelegramChat,
    text: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct TelegramChat {
    id: i64,
    #[serde(rename = "type")]
    chat_type: String,
    username: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest {
    chat_id: i64,
    text: String,
    parse_mode: String,
}

/// Gateway state — shared across all chats.
struct Gateway {
    bot_token: String,
    api_url: String,
    agents: Mutex<HashMap<i64, tokio::sync::mpsc::Sender<AgentMessage>>>,
    last_update_id: Mutex<i64>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl Gateway {
    fn new(token: String, data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            api_url: format!("https://api.telegram.org/bot{}", token),
            bot_token: token,
            agents: Mutex::new(HashMap::new()),
            last_update_id: Mutex::new(0),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

    /// Send a message to a Telegram chat.
    async fn send_message(&self, chat_id: i64, text: &str) -> anyhow::Result<()> {
        let url = format!("{}/sendMessage", self.api_url);
        let req = SendMessageRequest {
            chat_id,
            text: text.to_string(),
            parse_mode: "Markdown".to_string(),
        };

        let client = reqwest::Client::new();
        client.post(&url).json(&req).send().await?;
        Ok(())
    }

    /// Long-poll for updates.
    async fn get_updates(&self) -> anyhow::Result<Vec<TelegramUpdate>> {
        let mut last_id = self.last_update_id.lock().await;
        let url = format!(
            "{}/getUpdates?offset={}&timeout=30",
            self.api_url, *last_id + 1
        );

        let client = reqwest::Client::new();
        let resp: serde_json::Value = client.get(&url).send().await?.json().await?;

        let updates: Vec<TelegramUpdate> = if resp["ok"].as_bool().unwrap_or(false) {
            serde_json::from_value(resp["result"].clone()).unwrap_or_default()
        } else {
            warn!("Telegram API error: {:?}", resp);
            return Ok(vec![]);
        };

        // Update last_id
        for u in &updates {
            *last_id = (*last_id).max(u.update_id);
        }

        Ok(updates)
    }

    /// Create an agent for a chat if it doesn't exist.
    async fn get_or_create_agent(
        &self,
        chat_id: i64,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = self.agents.lock().await;

        if let Some(sender) = agents.get(&chat_id) {
            return Ok(sender.clone());
        }

        info!("Creating agent for chat {}", chat_id);

        // Session DB per chat
        let db_path = self.data_dir.join(format!("chat-{}.db", chat_id));
        let store = SessionStore::open(db_path.clone())?;

        // Memory store
        let memory_path = self.data_dir.join("memory.db");
        let memory_store = Arc::new(MemoryStore::open(memory_path)?);

        // Provider
        let provider = Box::new(OpenAIProvider::new(
            self.provider_api_key.clone(),
            self.model.clone(),
            self.provider_base_url.clone(),
        ));

        // Tools
        let mut tools = ToolRegistry::new();
        register_builtin_tools(&mut tools, Some(db_path), Some(memory_store), None);

        // Agent
        let agent_config = AgentConfig {
            name: format!("tg-{}", chat_id),
            session_id: None,
            system_prompt: Some(format!(
                "You are ARLI, an AI agent. You are communicating via Telegram. \
                 Current chat ID: {}. Respond in the user's language. \
                 Be concise — Telegram messages work best when brief.",
                chat_id
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
            PolicyEngine::default(),
            Some(store),
            20,
        );

        let sender = agent.sender();
        let gateway_tx = self.api_url.clone();
        let chat_id_copy = chat_id;

        // Spawn the agent's mailbox loop
        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        // Send response back via Telegram
                        let url = format!("{}/sendMessage", gateway_tx);
                        let req = SendMessageRequest {
                            chat_id: chat_id_copy,
                            text: if response.len() > 4000 {
                                format!("{}... _(truncated)_", &response[..4000])
                            } else {
                                response
                            },
                            parse_mode: "Markdown".to_string(),
                        };
                        if let Err(e) = reqwest::Client::new().post(&url).json(&req).send().await {
                            error!("Failed to send Telegram response: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Agent error for chat {}: {}", chat_id_copy, e);
                        // Agent died — remove from registry so it gets recreated
                        break;
                    }
                }
            }
        });

        agents.insert(chat_id, sender.clone());
        Ok(sender)
    }

    /// Process messages from incoming updates.
    async fn process_updates(&self, updates: Vec<TelegramUpdate>) {
        for update in updates {
            if let Some(msg) = update.message {
                let chat_id = msg.chat.id;
                let text = match msg.text {
                    Some(ref t) => t.clone(),
                    None => continue,
                };

                info!("TG message from {}: {}", chat_id, text);

                // Handle commands
                if text == "/start" {
                    if let Err(e) = self.send_message(chat_id, "🔥 ARLI agent is ready. Send a message to begin.").await {
                        error!("Send error: {}", e);
                    }
                    continue;
                }

                if text == "/help" {
                    let help = "ARLI Agent Harness\n\n\
                        Commands:\n\
                        /start — Initialize agent\n\
                        /help — This help\n\
                        /reset — Reset conversation\n\n\
                        Just send a message to start chatting!";
                    if let Err(e) = self.send_message(chat_id, help).await {
                        error!("Send error: {}", e);
                    }
                    continue;
                }

                if text == "/reset" {
                    self.agents.lock().await.remove(&chat_id);
                    if let Err(e) = self.send_message(chat_id, "Conversation reset. New session started.").await {
                        error!("Send error: {}", e);
                    }
                    continue;
                }

                // Route to agent
                match self.get_or_create_agent(chat_id).await {
                    Ok(sender) => {
                        if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                            error!("Failed to send to agent {}: {}", chat_id, e);
                            self.agents.lock().await.remove(&chat_id);
                        }
                    }
                    Err(e) => {
                        error!("Cannot create agent for {}: {}", chat_id, e);
                        let _ = self.send_message(chat_id, "⚠️ Agent initialization failed. Try again.").await;
                    }
                }
            }
        }
    }

    /// Main event loop — long-poll forever.
    async fn run_forever(self: Arc<Self>) {
        info!("ARLI Gateway starting...");

        loop {
            let updates = match self.get_updates().await {
                Ok(u) => u,
                Err(e) => {
                    error!("Poll error: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            if !updates.is_empty() {
                self.process_updates(updates).await;
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("ARLI_LOG")
                .unwrap_or_else(|_| "info,arli_gateway=debug".to_string()),
        )
        .init();

    let token = std::env::var("TELEGRAM_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("TELEGRAM_BOT_TOKEN env var not set"))?;

    let data_dir = std::env::var("ARLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs_next()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".arli")
        });

    std::fs::create_dir_all(&data_dir)?;

    let gateway = Arc::new(Gateway::new(token, data_dir)?);
    gateway.run_forever().await;

    Ok(())
}

fn dirs_next() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}
