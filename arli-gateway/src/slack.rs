//! Slack gateway — Socket Mode bot that routes messages to ARLI agents.
//!
//! Uses Slack's Socket Mode (WebSocket) — no public URL needed.
//! Reference: https://api.slack.com/apis/socket-mode

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
use tracing::{error, info, warn};

// ── Slack API types ──

#[derive(Debug, Deserialize)]
struct SocketModeResponse {
    ok: bool,
    url: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SlackEvent {
    #[serde(rename = "type")]
    event_type: String,
    envelope_id: Option<String>,
    payload: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SlackMessageEvent {
    #[serde(rename = "type")]
    event_type: String,
    channel: Option<String>,
    user: Option<String>,
    text: Option<String>,
    #[serde(rename = "bot_id")]
    bot_id: Option<String>,
    #[serde(rename = "thread_ts")]
    thread_ts: Option<String>,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct SlackAck {
    envelope_id: String,
}

#[derive(Debug, Serialize)]
struct ChatPostMessageRequest {
    channel: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<String>,
}

/// Per-channel agent entry.
struct SlackChannel {
    sender: tokio::sync::mpsc::Sender<AgentMessage>,
}

struct SlackBot {
    channels: Mutex<HashMap<String, SlackChannel>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    /// HTTP client for Slack API calls.
    http: reqwest::Client,
    bot_token: String,
}

impl SlackBot {
    fn new(bot_token: String, data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            channels: Mutex::new(HashMap::new()),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
            http: reqwest::Client::new(),
            bot_token,
        })
    }

    /// Call `apps.connections.open` to get a WebSocket URL.
    async fn open_socket(&self, app_token: &str) -> anyhow::Result<String> {
        let resp: SocketModeResponse = self
            .http
            .post("https://slack.com/api/apps.connections.open")
            .header("Authorization", format!("Bearer {}", app_token))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await?
            .json()
            .await?;

        if !resp.ok {
            anyhow::bail!("Slack apps.connections.open failed: {:?}", resp.error);
        }

        resp.url
            .ok_or_else(|| anyhow::anyhow!("No WebSocket URL returned"))
    }

    /// Post a message to a Slack channel.
    async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> anyhow::Result<()> {
        let req = ChatPostMessageRequest {
            channel: channel.to_string(),
            text: text.to_string(),
            thread_ts: thread_ts.map(|t| t.to_string()),
        };

        let resp: serde_json::Value = self
            .http
            .post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&req)
            .send()
            .await?
            .json()
            .await?;

        if !resp["ok"].as_bool().unwrap_or(false) {
            warn!("Slack postMessage error: {:?}", resp.get("error"));
        }

        Ok(())
    }

    async fn get_or_create_agent(
        &self,
        channel_id: &str,
        channel_name: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut channels = self.channels.lock().await;

        if let Some(ch) = channels.get(channel_id) {
            return Ok(ch.sender.clone());
        }

        info!("Creating Slack agent for {}", channel_id);

        let db_path = self.data_dir.join(format!("slack-{}.db", channel_id));
        let store = SessionStore::open(db_path.clone())?;
        let memory_path = self.data_dir.join("memory.db");
        let memory_store = Arc::new(MemoryStore::open(memory_path)?);

        let provider = Box::new(OpenAIProvider::new(
            self.provider_api_key.clone(),
            self.model.clone(),
            self.provider_base_url.clone(),
        ));

        let mut tools = ToolRegistry::new();
        register_builtin_tools(&mut tools, Some(db_path), Some(memory_store), None, None, None);

        let system_prompt = format!(
            "You are ARLI, an AI agent communicating via Slack in channel '{}'. \
             Respond in the user's language. Be concise. \
             Slack messages support mrkdwn formatting.",
            channel_name
        );

        let agent_config = AgentConfig {
            name: format!("slack-{}", channel_id),
            session_id: None,
            system_prompt: Some(system_prompt),
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
        let channel_id_owned = channel_id.to_string();

        // For Slack, we need to forward responses — but the agent runs in a
        // spawned task. We collect responses through a dedicated channel.
        let (response_tx, mut response_rx) = tokio::sync::mpsc::channel::<String>(32);

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        let _ = response_tx.send(response).await;
                    }
                    Err(e) => {
                        error!("Slack agent {} error: {}", channel_id_owned, e);
                        break;
                    }
                }
            }
        });

        // Spawn response handler
        let http = self.http.clone();
        let bot_token = self.bot_token.clone();
        let ch_id = channel_id.to_string();
        tokio::spawn(async move {
            while let Some(response) = response_rx.recv().await {
                if response.is_empty() {
                    continue;
                }
                let text = if response.len() > 3000 {
                    format!("{}... _(truncated)_", &response[..2990])
                } else {
                    response
                };

                let req = ChatPostMessageRequest {
                    channel: ch_id.clone(),
                    text,
                    thread_ts: None,
                };

                if let Err(e) = http
                    .post("https://slack.com/api/chat.postMessage")
                    .header("Authorization", format!("Bearer {}", bot_token))
                    .json(&req)
                    .send()
                    .await
                {
                    error!("Failed to send Slack response: {}", e);
                }
            }
        });

        channels.insert(
            channel_id.to_string(),
            SlackChannel {
                sender: sender.clone(),
            },
        );
        Ok(sender)
    }

    async fn remove_agent(&self, channel_id: &str) {
        self.channels.lock().await.remove(channel_id);
        info!("Removed Slack agent for {}", channel_id);
    }
}

/// Send an acknowledgement for a Slack socket mode event.
async fn ack_envelope(
    ws_tx: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    envelope_id: &str,
) {
    let ack = serde_json::json!({
        "envelope_id": envelope_id,
    });
    if let Err(e) = ws_tx.send(WsMessage::Text(ack.to_string())).await {
        error!("Failed to ack Slack envelope: {}", e);
    }
}

/// Run the Slack gateway in Socket Mode.
pub async fn run(bot_token: String, app_token: String, data_dir: PathBuf) -> anyhow::Result<()> {
    info!("Slack gateway starting...");

    let bot = Arc::new(SlackBot::new(bot_token.clone(), data_dir)?);

    // Get WebSocket URL via apps.connections.open
    let ws_url = bot.open_socket(&app_token).await?;
    info!("Slack socket URL obtained");

    let (ws_stream, _) = connect_async(&ws_url).await?;
    info!("Slack WebSocket connected");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Process incoming messages
    while let Some(msg) = ws_rx.next().await {
        match msg {
            Ok(WsMessage::Text(text)) => {
                let event: SlackEvent = match serde_json::from_str(&text) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("Failed to parse Slack event: {}", e);
                        continue;
                    }
                };

                // Acknowledge the envelope
                let envelope_id = event.envelope_id.clone();

                match event.event_type.as_str() {
                    "hello" => {
                        info!("Slack socket connected successfully");
                        if let Some(eid) = &envelope_id {
                            ack_envelope(&mut ws_tx, eid).await;
                        }
                    }
                    "disconnect" => {
                        warn!("Slack requested disconnect, reconnecting...");
                        if let Some(eid) = &envelope_id {
                            ack_envelope(&mut ws_tx, eid).await;
                        }
                        // Reconnect
                        match bot.open_socket(&app_token).await {
                            Ok(url) => {
                                if let Ok((new_ws, _)) = connect_async(&url).await {
                                    let (new_tx, new_rx) = new_ws.split();
                                    ws_tx = new_tx;
                                    ws_rx = new_rx;
                                    info!("Slack reconnected");
                                }
                            }
                            Err(e) => error!("Slack reconnect failed: {}", e),
                        }
                    }
                    "events_api" => {
                        // Acknowledge first
                        if let Some(eid) = &envelope_id {
                            ack_envelope(&mut ws_tx, eid).await;
                        }

                        // Process the inner event
                        if let Some(payload) = &event.payload {
                            let msg_event: SlackMessageEvent =
                                match serde_json::from_value(payload.clone()) {
                                    Ok(e) => e,
                                    Err(_) => continue,
                                };

                            if msg_event.event_type != "message" {
                                continue;
                            }

                            // Skip bot messages
                            if msg_event.bot_id.is_some() {
                                continue;
                            }

                            let channel = match &msg_event.channel {
                                Some(c) => c.clone(),
                                None => continue,
                            };

                            let text = match &msg_event.text {
                                Some(t) => t.trim().to_string(),
                                None => continue,
                            };

                            if text.is_empty() {
                                continue;
                            }

                            let user = msg_event.user.as_deref().unwrap_or("unknown");
                            let thread_ts = msg_event.thread_ts.clone();

                            info!("Slack message from {} in {}: {}", user, channel, text);

                            // Handle commands
                            if text == "!arli start" || text == "!start" {
                                let _ = bot
                                    .post_message(
                                        &channel,
                                        "ARLI agent is ready.",
                                        thread_ts.as_deref(),
                                    )
                                    .await;
                                continue;
                            }

                            if text == "!arli reset" || text == "!reset" {
                                bot.remove_agent(&channel).await;
                                let _ = bot
                                    .post_message(
                                        &channel,
                                        "Conversation reset.",
                                        thread_ts.as_deref(),
                                    )
                                    .await;
                                continue;
                            }

                            // Route to agent
                            let channel_name = format!("#{}", channel);
                            match bot.get_or_create_agent(&channel, &channel_name).await {
                                Ok(sender) => {
                                    if let Err(e) =
                                        sender.send(AgentMessage::UserMessage(text)).await
                                    {
                                        error!("Failed to send to Slack agent {}: {}", channel, e);
                                        bot.remove_agent(&channel).await;
                                    }
                                }
                                Err(e) => {
                                    error!("Cannot create Slack agent for {}: {}", channel, e);
                                    let _ = bot
                                        .post_message(
                                            &channel,
                                            "Agent init failed.",
                                            thread_ts.as_deref(),
                                        )
                                        .await;
                                }
                            }
                        }
                    }
                    _ => {
                        // Unknown event type — still ack
                        if let Some(eid) = &envelope_id {
                            ack_envelope(&mut ws_tx, eid).await;
                        }
                    }
                }
            }
            Ok(WsMessage::Close(_)) => {
                warn!("Slack WebSocket closed, reconnecting...");
                match bot.open_socket(&app_token).await {
                    Ok(url) => {
                        if let Ok((new_ws, _)) = connect_async(&url).await {
                            let (new_tx, new_rx) = new_ws.split();
                            ws_tx = new_tx;
                            ws_rx = new_rx;
                        }
                    }
                    Err(e) => error!("Slack reconnect failed: {}", e),
                }
            }
            Err(e) => {
                error!("Slack WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    Ok(())
}
