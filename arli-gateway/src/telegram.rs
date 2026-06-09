//! Telegram gateway — long-polls Telegram Bot API for messages,
//! routes each chat to a dedicated ARLI Agent.

use arli_core::policy::{Decision, PolicyEngine, PolicyRule};
use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::pairing;

// ── Telegram API types ──

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
    callback_query: Option<TelegramCallback>,
}

#[derive(Debug, Deserialize)]
struct TelegramCallback {
    id: String,
    data: Option<String>,
    message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    #[allow(dead_code)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<InlineKeyboardMarkup>,
}

#[derive(Debug, Serialize)]
struct InlineKeyboardMarkup {
    inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}

#[derive(Debug, Serialize)]
struct InlineKeyboardButton {
    text: String,
    callback_data: String,
}

#[derive(Debug, Serialize)]
struct AnswerCallbackRequest {
    callback_query_id: String,
    text: Option<String>,
}

struct TelegramGateway {
    api_url: String,
    agents: Mutex<HashMap<i64, tokio::sync::mpsc::Sender<AgentMessage>>>,
    pending_approvals: Mutex<HashMap<i64, tokio::sync::oneshot::Sender<bool>>>,
    last_update_id: Mutex<i64>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    allowed_users: Mutex<pairing::AllowedUsers>,
}

impl TelegramGateway {
    fn new(token: String, data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        let allowed = pairing::AllowedUsers::load(&data_dir);
        Ok(Self {
            api_url: format!("https://api.telegram.org/bot{}", token),
            agents: Mutex::new(HashMap::new()),
            pending_approvals: Mutex::new(HashMap::new()),
            last_update_id: Mutex::new(0),
            data_dir: data_dir.clone(),
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
            allowed_users: Mutex::new(allowed),
        })
    }

    async fn send_chat_action(&self, chat_id: i64, action: &str) {
        let url = format!("{}/sendChatAction", self.api_url);
        let body = serde_json::json!({"chat_id": chat_id, "action": action});
        let _ = reqwest::Client::new().post(&url).json(&body).send().await;
    }

    async fn send_message(&self, chat_id: i64, text: &str) -> anyhow::Result<()> {
        let url = format!("{}/sendMessage", self.api_url);
        let req = SendMessageRequest {
            chat_id,
            text: text.to_string(),
            parse_mode: "Markdown".to_string(),
            reply_markup: None,
        };
        let client = reqwest::Client::new();
        client.post(&url).json(&req).send().await?;
        Ok(())
    }

    async fn send_approval_request(
        &self,
        chat_id: i64,
        tool_name: &str,
        reason: &str,
    ) -> anyhow::Result<()> {
        let url = format!("{}/sendMessage", self.api_url);
        let markup = InlineKeyboardMarkup {
            inline_keyboard: vec![vec![
                InlineKeyboardButton {
                    text: "Approve".to_string(),
                    callback_data: format!("approve:{}", tool_name),
                },
                InlineKeyboardButton {
                    text: "Deny".to_string(),
                    callback_data: format!("deny:{}", tool_name),
                },
            ]],
        };
        let req = SendMessageRequest {
            chat_id,
            text: format!(
                "*Approval required*: {}\n\nTool: `{}`\nReason: {}",
                "\u{26a0}\u{fe0f}", tool_name, reason
            ),
            parse_mode: "Markdown".to_string(),
            reply_markup: Some(markup),
        };
        let client = reqwest::Client::new();
        client.post(&url).json(&req).send().await?;
        Ok(())
    }

    async fn answer_callback(&self, callback_id: &str, text: &str) {
        let url = format!("{}/answerCallbackQuery", self.api_url);
        let req = AnswerCallbackRequest {
            callback_query_id: callback_id.to_string(),
            text: Some(text.to_string()),
        };
        let _ = reqwest::Client::new().post(&url).json(&req).send().await;
    }

    async fn get_updates(&self) -> anyhow::Result<Vec<TelegramUpdate>> {
        let mut last_id = self.last_update_id.lock().await;
        let url = format!(
            "{}/getUpdates?offset={}&timeout=30",
            self.api_url,
            *last_id + 1
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

    fn make_permissive_policy() -> PolicyEngine {
        let mut engine = PolicyEngine::new();
        // Allow all tools for trusted Telegram user
        engine.add_rule(PolicyRule {
            name: "allow-all-telegram".into(),
            tool_match: None, // matches all
            toolset: None,
            decision: Decision::Allow,
            priority: 1000,
            agent_profile: None,
            description: "Telegram user is trusted — allow all tools".into(),
        });
        engine
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
            name: format!("tg-{}", chat_id),
            session_id: None,
            system_prompt: Some(format!(
                "You are ARLI, an AI agent communicating via Telegram. \
                 Current chat ID: {}. Respond in the user's language. Be concise.\n\n\
                 CRITICAL: You have FULL permissions to write files, execute shell commands, \
                 and create anything the user asks. NEVER ask for approval, permission, \
                 or confirmation. Just DO what the user asks. \
                 If a tool returns an error, try a different approach. \
                 Never say 'I need permission' or 'Approve this' — you already have it.",
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
            Self::make_permissive_policy(),
            Some(store),
            20,
            None,
        );

        let sender = agent.sender();
        let api_url = self.api_url.clone();
        let chat_id_clone = chat_id;

        tokio::spawn(async move {
            loop {
                // Send typing indicator
                let action_url = format!("{}/sendChatAction", api_url);
                let action_body = serde_json::json!({"chat_id": chat_id_clone, "action": "typing"});
                let _ = reqwest::Client::new()
                    .post(&action_url)
                    .json(&action_body)
                    .send()
                    .await;

                match agent.run(None).await {
                    Ok(response) => {
                        // Check for approval requests in the response
                        let needs_approval = response.contains("APPROVAL REQUIRED");

                        let url = format!("{}/sendMessage", api_url);
                        let text = if response.len() > 4000 {
                            format!("{}... _(truncated)_", &response[..4000])
                        } else {
                            response.clone()
                        };

                        // Build message with inline keyboard if approval needed
                        let reply_markup = if needs_approval {
                            Some(InlineKeyboardMarkup {
                                inline_keyboard: vec![vec![
                                    InlineKeyboardButton {
                                        text: "Approve".to_string(),
                                        callback_data: "approve:generic".to_string(),
                                    },
                                    InlineKeyboardButton {
                                        text: "Deny".to_string(),
                                        callback_data: "deny:generic".to_string(),
                                    },
                                ]],
                            })
                        } else {
                            None
                        };

                        let req = SendMessageRequest {
                            chat_id: chat_id_clone,
                            text,
                            parse_mode: "Markdown".to_string(),
                            reply_markup,
                        };
                        if let Err(e) = reqwest::Client::new().post(&url).json(&req).send().await {
                            error!("Failed to send Telegram response: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Agent error for chat {}: {}", chat_id_clone, e);
                        let url = format!("{}/sendMessage", api_url);
                        let req = SendMessageRequest {
                            chat_id: chat_id_clone,
                            text: format!("Error: {}", e),
                            parse_mode: "Markdown".to_string(),
                            reply_markup: None,
                        };
                        let _ = reqwest::Client::new().post(&url).json(&req).send().await;
                    }
                }
            }
        });

        agents.insert(chat_id, sender.clone());
        Ok(sender)
    }

    async fn handle_callback(&self, callback: &TelegramCallback) {
        let chat_id = match &callback.message {
            Some(msg) => msg.chat.id,
            None => return,
        };

        let data = match &callback.data {
            Some(d) => d.as_str(),
            None => return,
        };

        let approved = data.starts_with("approve:");
        let status = if approved { "Approved" } else { "Denied" };

        // Answer the callback to remove loading state
        self.answer_callback(&callback.id, status).await;

        // Send confirmation message
        let tool = data.split(':').nth(1).unwrap_or("unknown");
        let text = if approved {
            format!("Approved: `{}` — proceeding.", tool)
        } else {
            format!("Denied: `{}` — blocked.", tool)
        };
        let _ = self.send_message(chat_id, &text).await;

        // Resolve pending approval if any
        let sender = {
            let mut pending = self.pending_approvals.lock().await;
            pending.remove(&chat_id)
        };
        if let Some(tx) = sender {
            let _ = tx.send(approved);
        }
    }
}

/// Public entry point — starts the Telegram long-poll loop.
pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let token = std::env::var("TELEGRAM_BOT_TOKEN")
        .ok()
        .or_else(|| {
            let config_path = data_dir.join("config.toml");
            if config_path.exists() {
                std::fs::read_to_string(&config_path).ok().and_then(|s| {
                    toml::from_str::<toml::Value>(&s).ok().and_then(|v| {
                        v.get("gateway")?
                            .get("telegram_token")
                            .or_else(|| v.get("gateway")?.get("bot_token"))
                            .and_then(|val| val.as_str().map(String::from))
                    })
                })
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow::anyhow!("TELEGRAM_BOT_TOKEN not set"))?;

    let gateway = TelegramGateway::new(token, data_dir)?;

    info!("Telegram gateway started — long-polling for updates");

    loop {
        match gateway.get_updates().await {
            Ok(updates) => {
                for update in updates {
                    if let Some(ref callback) = update.callback_query {
                        gateway.handle_callback(callback).await;
                    }
                    if let Some(ref msg) = update.message {
                        if let Some(ref text) = msg.text {
                            // ── Authorisation gate ──
                            let is_allowed = gateway.allowed_users.lock().await.is_allowed(msg.chat.id);

                            if !is_allowed {
                                // Check for pairing command: "arli pair <code>"
                                let pair_prefix = "arli pair ";
                                if let Some(code) = text.strip_prefix(pair_prefix) {
                                    let code = code.trim();
                                    if pairing::verify(code, &gateway.data_dir) {
                                        gateway.allowed_users
                                            .lock()
                                            .await
                                            .add(msg.chat.id, &gateway.data_dir)?;
                                        info!("Chat {} paired successfully", msg.chat.id);
                                        let _ = gateway.send_message(
                                            msg.chat.id,
                                            "Paired successfully! ARLI at your service. Send me any message to begin.",
                                        ).await;
                                    } else {
                                        let _ = gateway.send_message(
                                            msg.chat.id,
                                            "Invalid or expired pairing code. Run `arli pair generate` on the server to get a new code.",
                                        ).await;
                                    }
                                } else {
                                    let _ = gateway.send_message(
                                        msg.chat.id,
                                        "Unauthorized. Run `arli pair generate` on the server and send the code:\n\narli pair <code>",
                                    ).await;
                                }
                                continue;
                            }

                            // ── Allowed user — dispatch normally ──

                            if text == "/start" {
                                let _ = gateway
                                    .send_message(
                                        msg.chat.id,
                                        "ARLI at your service. Send me any message to begin.",
                                    )
                                    .await;
                                continue;
                            }

                            let sender = gateway.get_or_create_agent(msg.chat.id).await?;
                            let user_msg = AgentMessage::UserMessage(text.clone());
                            if let Err(e) = sender.send(user_msg).await {
                                error!(
                                    "Failed to send message to agent for chat {}: {}",
                                    msg.chat.id, e
                                );
                                let _ = gateway
                                    .send_message(
                                        msg.chat.id,
                                        &format!("Error processing message: {}", e),
                                    )
                                    .await;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("Telegram polling error: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    }
}
