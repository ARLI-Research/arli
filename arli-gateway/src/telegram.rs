//! Telegram gateway — long-polls Telegram Bot API for messages,
//! routes each chat to a dedicated ARLI Agent.

use arli_core::{
    Agent, AgentConfig, AgentMessage, Config,
    OpenAIProvider, SessionStore, ToolRegistry,
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
    #[allow(dead_code)]
    chat_type: String,
    #[allow(dead_code)]
    username: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest {
    chat_id: i64,
    text: String,
    parse_mode: String,
}

struct TelegramGateway {
    api_url: String,
    agents: Mutex<HashMap<i64, tokio::sync::mpsc::Sender<AgentMessage>>>,
    last_update_id: Mutex<i64>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl TelegramGateway {
    fn new(token: String, data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            api_url: format!("https://api.telegram.org/bot{}", token),
            agents: Mutex::new(HashMap::new()),
            last_update_id: Mutex::new(0),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

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

        for u in &updates {
            *last_id = (*last_id).max(u.update_id);
        }

        Ok(updates)
    }

    async fn get_or_create_agent(
        &self,
        chat_id: i64,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = self.agents.lock().await;

        if let Some(sender) = agents.get(&chat_id) {
            return Ok(sender.clone());
        }

        info!("Creating Telegram agent for chat {}", chat_id);

        let db_path = self.data_dir.join(format!("chat-{}.db", chat_id));
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
            name: format!("tg-{}", chat_id),
            session_id: None,
            system_prompt: Some(format!(
                "You are ARLI, an AI agent communicating via Telegram. \
                 Current chat ID: {}. Respond in the user's language. Be concise.",
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
            arli_core::PolicyEngine::default(),
            Some(store),
            20,
        );

        let sender = agent.sender();
        let api_url = self.api_url.clone();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        let url = format!("{}/sendMessage", api_url);
                        let req = SendMessageRequest {
                            chat_id,
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
                        error!("Telegram agent error for chat {}: {}", chat_id, e);
                        break;
                    }
                }
            }
        });

        agents.insert(chat_id, sender.clone());
        Ok(sender)
    }

    async fn process_updates(&self, updates: Vec<TelegramUpdate>) {
        for update in updates {
            if let Some(msg) = update.message {
                let chat_id = msg.chat.id;
                let text = match msg.text {
                    Some(ref t) => t.clone(),
                    None => continue,
                };

                info!("TG message from {}: {}", chat_id, text);

                if text == "/start" {
                    let _ = self.send_message(chat_id, "ARLI agent is ready. Send a message to begin.").await;
                    continue;
                }

                if text == "/help" {
                    let help = "ARLI Agent Harness\n\n\
                        Commands:\n\
                        /start — Initialize agent\n\
                        /help — This help\n\
                        /reset — Reset conversation";
                    let _ = self.send_message(chat_id, help).await;
                    continue;
                }

                if text == "/reset" {
                    self.agents.lock().await.remove(&chat_id);
                    let _ = self.send_message(chat_id, "Conversation reset. New session started.").await;
                    continue;
                }

                match self.get_or_create_agent(chat_id).await {
                    Ok(sender) => {
                        if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                            error!("Failed to send to agent {}: {}", chat_id, e);
                            self.agents.lock().await.remove(&chat_id);
                        }
                    }
                    Err(e) => {
                        error!("Cannot create agent for {}: {}", chat_id, e);
                        let _ = self.send_message(chat_id, "Agent initialization failed.").await;
                    }
                }
            }
        }
    }

    async fn run_forever(self: Arc<Self>) {
        info!("Telegram gateway starting...");
        loop {
            let updates = match self.get_updates().await {
                Ok(u) => u,
                Err(e) => {
                    error!("Telegram poll error: {}", e);
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

/// Read the Telegram bot token from env or config file.
fn resolve_telegram_token() -> Option<String> {
    std::env::var("TELEGRAM_BOT_TOKEN").ok().or_else(|| {
        let config_path = std::env::var("ARLI_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".arli"))
                    .unwrap_or_else(|_| PathBuf::from(".arli"))
            })
            .join("config.toml");
        if config_path.exists() {
            std::fs::read_to_string(&config_path).ok().and_then(|s| {
                toml::from_str::<toml::Value>(&s).ok().and_then(|v| {
                    v.get("gateway")?.get("telegram_token")?.as_str().map(String::from)
                })
            })
        } else {
            None
        }
    })
}

pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let token = resolve_telegram_token()
        .ok_or_else(|| anyhow::anyhow!(
            "TELEGRAM_BOT_TOKEN not set. Set TELEGRAM_BOT_TOKEN env var or \
             gateway.telegram_token in config.toml"
        ))?;

    let gateway = Arc::new(TelegramGateway::new(token, data_dir)?);
    gateway.run_forever().await;
    Ok(())
}
