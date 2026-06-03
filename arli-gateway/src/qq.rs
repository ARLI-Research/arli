//! QQ Bot gateway — WebSocket-based using QQ official bot API.
//!
//! Connects via WebSocket to api.sgroup.qq.com/gateway for real-time events.
//! Handles message events (AT_MESSAGE_CREATE, DIRECT_MESSAGE_CREATE, etc.).
//! Replies via QQ REST API (POST /v2/users/{openid}/messages or /channels/{id}/messages).
//!
//! Environment variables:
//!   QQ_APP_ID    — QQ Bot app ID
//!   QQ_BOT_TOKEN — QQ Bot access token
//!
//! Reference: https://bot.q.qq.com/wiki/develop/api-v2/

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

// ── QQ Gateway payload types ──

#[derive(Debug, Deserialize)]
struct GatewayPayload {
    op: i32,
    d: Option<Value>,
    s: Option<i64>,
    t: Option<String>,
}

#[derive(Debug, Serialize)]
struct IdentifyPayload {
    op: i32,
    d: IdentifyData,
}

#[derive(Debug, Serialize)]
struct IdentifyData {
    token: String,
    intents: i32,
    shard: Vec<i32>,
    properties: Value,
}

#[derive(Debug, Serialize)]
struct HeartbeatPayload {
    op: i32,
    d: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MessageEvent {
    id: Option<String>,
    author: Option<AuthorInfo>,
    content: Option<String>,
    guild_id: Option<String>,
    channel_id: Option<String>,
    group_openid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorInfo {
    id: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageReq {
    content: String,
    msg_type: i32,
    msg_id: Option<String>,
}

struct QqGateway {
    app_id: String,
    bot_token: String,
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl QqGateway {
    fn new(app_id: String, bot_token: String, data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            app_id,
            bot_token,
            agents: Mutex::new(HashMap::new()),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

    /// Send a message to a QQ channel or direct user.
    async fn send_message(
        app_id: &str,
        bot_token: &str,
        chat_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let truncated = if text.len() > 2000 {
            format!("{}... _(truncated)_", &text[..1990])
        } else {
            text.to_string()
        };

        let client = reqwest::Client::new();

        // chat_id can be "guild:channel" or just "user_openid"
        let url = if chat_id.contains(':') {
            // Group channel: chat_id = "guild_id:channel_id"
            let parts: Vec<&str> = chat_id.split(':').collect();
            format!(
                "https://api.sgroup.qq.com/v2/guilds/{}/channels/{}/messages",
                parts[0], parts[1]
            )
        } else {
            // DM: chat_id = user openid
            format!("https://api.sgroup.qq.com/v2/users/{}/messages", chat_id)
        };

        let req = SendMessageReq {
            content: truncated,
            msg_type: 0,
            msg_id: None,
        };

        let resp = client
            .post(&url)
            .header("Authorization", format!("QQBot {}", bot_token))
            .header("X-Union-Appid", app_id)
            .json(&req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            warn!(
                "QQ send error ({}): {}",
                status,
                resp.text().await.unwrap_or_default()
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
        chat_id: &str,
        app_id: &str,
        bot_token: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        if let Some(sender) = agents.get(chat_id) {
            return Ok(sender.clone());
        }

        let safe_id = chat_id.replace([':', '/', ' ', '@', '\\'], "_");
        info!("Creating QQ agent for chat {}", chat_id);

        let db_path = data_dir.join(format!("qq-{}.db", safe_id));
        let store = SessionStore::open(db_path.clone())?;
        let memory_path = data_dir.join("memory.db");
        let memory_store = Arc::new(MemoryStore::open(memory_path)?);

        let provider = Box::new(OpenAIProvider::new(
            provider_api_key.to_string(),
            model.to_string(),
            provider_base_url.clone(),
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
            name: format!("qq-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via QQ. \
                 Respond in the user's language. Be concise."
                    .to_string(),
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
        let chat_owned = chat_id.to_string();
        let app_owned = app_id.to_string();
        let token_owned = bot_token.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        let truncated = if response.len() > 2000 {
                            format!("{}... _(truncated)_", &response[..1990])
                        } else {
                            response
                        };
                        if let Err(e) =
                            Self::send_message(&app_owned, &token_owned, &chat_owned, &truncated)
                                .await
                        {
                            error!("Failed to send QQ response: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("QQ agent error for chat {}: {}", chat_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(chat_id.to_string(), sender.clone());
        Ok(sender)
    }

    async fn handle_message_event(&self, event: &MessageEvent) {
        let author_id = match &event.author {
            Some(a) => a.id.as_deref().unwrap_or("unknown"),
            None => "unknown",
        };

        let text = match &event.content {
            Some(t) => t.trim().to_string(),
            None => return,
        };

        if text.is_empty() {
            return;
        }

        // Determine chat key: "guild_id:channel_id" for groups, author openid for DMs
        let chat_id = if let (Some(gid), Some(cid)) = (&event.guild_id, &event.channel_id) {
            format!("{}:{}", gid, cid)
        } else if let Some(openid) = &event.group_openid {
            openid.clone()
        } else {
            author_id.to_string()
        };

        info!("QQ message from {} (chat {}): {}", author_id, chat_id, text);

        match Self::get_or_create_agent(
            &self.agents,
            &self.data_dir,
            &self.provider_api_key,
            &self.provider_base_url,
            &self.model,
            &chat_id,
            &self.app_id,
            &self.bot_token,
        )
        .await
        {
            Ok(sender) => {
                if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                    error!("Failed to send to QQ agent {}: {}", chat_id, e);
                    self.agents.lock().await.remove(&chat_id);
                }
            }
            Err(e) => {
                error!("Cannot create QQ agent for {}: {}", chat_id, e);
            }
        }
    }

    async fn run_forever(self: Arc<Self>) -> anyhow::Result<()> {
        info!("QQ Bot gateway connecting to WebSocket...");

        let ws_url = "wss://api.sgroup.qq.com/gateway";
        let (ws_stream, _) = connect_async(ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        // Wait for Hello (op=10)
        let hello: Option<i64> = loop {
            if let Some(Ok(Message::Text(text))) = read.next().await {
                if let Ok(payload) = serde_json::from_str::<GatewayPayload>(&text) {
                    if payload.op == 10 {
                        break payload
                            .d
                            .and_then(|d| d.get("heartbeat_interval").and_then(|v| v.as_i64()));
                    }
                }
            }
        };

        let heartbeat_ms = hello.unwrap_or(41250);
        info!(
            "QQ gateway connected, heartbeat interval: {}ms",
            heartbeat_ms
        );

        // Send Identify (op=2)
        let identify = IdentifyPayload {
            op: 2,
            d: IdentifyData {
                token: format!("QQBot {}", self.bot_token),
                intents: 1 << 9 | 1 << 12, // GROUP_AT_MESSAGE | DIRECT_MESSAGE
                shard: vec![0, 1],
                properties: serde_json::json!({}),
            },
        };
        write
            .send(Message::Text(serde_json::to_string(&identify)?))
            .await?;

        let last_seq: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
        let heartbeat = heartbeat_ms as u64;

        // Spawn heartbeat
        let (heartbeat_tx, mut heartbeat_rx) = tokio::sync::mpsc::channel::<()>(1);
        let mut hb_write = write;
        let hb_seq = last_seq.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(heartbeat)).await;
                if heartbeat_rx.try_recv().is_ok() {
                    return;
                }
                let seq = *hb_seq.lock().await;
                let hb = HeartbeatPayload { op: 1, d: seq };
                if let Ok(text) = serde_json::to_string(&hb) {
                    let _ = hb_write.send(Message::Text(text)).await;
                }
            }
        });

        // Read loop
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let payload: GatewayPayload = match serde_json::from_str(&text) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };

                    if let Some(ref s) = payload.s {
                        *last_seq.lock().await = Some(*s);
                    }

                    match payload.op {
                        0 => {
                            // Dispatch event
                            let event_type = payload.t.as_deref().unwrap_or("");
                            if let Some(ref d) = payload.d {
                                if event_type == "GROUP_AT_MESSAGE_CREATE"
                                    || event_type == "DIRECT_MESSAGE_CREATE"
                                    || event_type == "AT_MESSAGE_CREATE"
                                    || event_type == "MESSAGE_CREATE"
                                {
                                    if let Ok(event) =
                                        serde_json::from_value::<MessageEvent>(d.clone())
                                    {
                                        self.handle_message_event(&event).await;
                                    }
                                }
                            }
                        }
                        11 => {
                            // Heartbeat ACK
                        }
                        _ => {}
                    }
                }
                Ok(Message::Close(_)) => {
                    warn!("QQ WebSocket closed, reconnecting...");
                    break;
                }
                Err(e) => {
                    error!("QQ WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        // Signal heartbeat to stop
        let _ = heartbeat_tx.send(());
        Ok(())
    }
}

pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let app_id = std::env::var("QQ_APP_ID")
        .ok()
        .ok_or_else(|| anyhow::anyhow!("QQ_APP_ID not set. Set QQ_APP_ID env var."))?;

    let bot_token = std::env::var("QQ_BOT_TOKEN")
        .ok()
        .ok_or_else(|| anyhow::anyhow!("QQ_BOT_TOKEN not set. Set QQ_BOT_TOKEN env var."))?;

    // Reconnect loop
    loop {
        let gateway = Arc::new(QqGateway::new(
            app_id.clone(),
            bot_token.clone(),
            data_dir.clone(),
        )?);

        if let Err(e) = gateway.run_forever().await {
            error!("QQ gateway error: {}. Reconnecting in 5s...", e);
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }
}
