//! LINE gateway — webhook receiver + message sender.
//!
//! Uses LINE Messaging API (api.line.me).
//!
//! Environment variables:
//!   LINE_CHANNEL_SECRET       — LINE channel secret
//!   LINE_CHANNEL_ACCESS_TOKEN — LINE channel access token
//!   LINE_PORT                 — Webhook listen port (default: 3012)
//!
//! Reference: https://developers.line.biz/en/reference/messaging-api/

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

// ── LINE Messaging API types ──

/// Inbound webhook body from LINE.
#[derive(Debug, Deserialize)]
struct LineWebhook {
    destination: Option<String>,
    events: Option<Vec<LineEvent>>,
}

#[derive(Debug, Deserialize)]
struct LineEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    mode: Option<String>,
    timestamp: Option<i64>,
    source: Option<LineSource>,
    message: Option<LineMessage>,
    #[serde(rename = "replyToken")]
    reply_token: Option<String>,
    #[serde(rename = "webhookEventId")]
    #[allow(dead_code)]
    webhook_event_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LineSource {
    #[serde(rename = "type")]
    source_type: Option<String>,
    #[serde(rename = "userId")]
    user_id: Option<String>,
    #[serde(rename = "groupId")]
    #[allow(dead_code)]
    group_id: Option<String>,
    #[serde(rename = "roomId")]
    #[allow(dead_code)]
    room_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LineMessage {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    id: Option<String>,
    text: Option<String>,
}

/// Outbound reply message.
#[derive(Debug, Serialize)]
struct ReplyRequest {
    #[serde(rename = "replyToken")]
    reply_token: String,
    messages: Vec<ReplyMessage>,
}

#[derive(Debug, Serialize)]
struct ReplyMessage {
    #[serde(rename = "type")]
    msg_type: String,
    text: String,
}

/// Shared state for LINE webhook handler.
struct LineState {
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    response_tx: tokio::sync::mpsc::Sender<(String, String, String)>, // (reply_token, user_id, text)
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    channel_access_token: String,
}

impl LineState {
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        user_id: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        let safe_id = user_id.replace([':', '/', '\\', ' ', '@'], "_");

        if let Some(sender) = agents.get(user_id) {
            return Ok(sender.clone());
        }

        info!("Creating LINE agent for user {}", user_id);

        let db_path = data_dir.join(format!("line-{}.db", safe_id));
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
            name: format!("line-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via LINE. \
                 Respond in the user's language. Be concise — LINE messages \
                 work best when kept short and friendly."
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
        let user_owned = user_id.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(_response) => {
                        // Response handled via response channel
                    }
                    Err(e) => {
                        error!("LINE agent {} error: {}", user_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(user_id.to_string(), sender.clone());
        Ok(sender)
    }
}

// ── LINE API helpers ──

/// Send a reply message via LINE Messaging API.
async fn send_line_reply(
    channel_access_token: &str,
    reply_token: &str,
    text: &str,
) -> anyhow::Result<()> {
    // LINE messages are limited to 5000 characters
    let truncated = if text.len() > 5000 {
        format!("{}...", &text[..4997])
    } else {
        text.to_string()
    };

    let req = ReplyRequest {
        reply_token: reply_token.to_string(),
        messages: vec![ReplyMessage {
            msg_type: "text".to_string(),
            text: truncated,
        }],
    };

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.line.me/v2/bot/message/reply")
        .header("Authorization", format!("Bearer {}", channel_access_token))
        .json(&req)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        warn!("LINE send error ({}): {}", status, body);
    }

    Ok(())
}

// ── Webhook handler ──

async fn line_webhook(
    State(state): State<Arc<LineState>>,
    Json(body): Json<LineWebhook>,
) -> impl IntoResponse {
    let events = match body.events {
        Some(e) => e,
        None => return "ok",
    };

    for event in events {
        let event_type = event.event_type.as_deref().unwrap_or("");

        if event_type != "message" {
            continue;
        }

        let message = match &event.message {
            Some(m) => m,
            None => continue,
        };

        // Only handle text messages
        if message.msg_type.as_deref() != Some("text") {
            continue;
        }

        let text = match &message.text {
            Some(t) => t.trim().to_string(),
            None => continue,
        };

        if text.is_empty() {
            continue;
        }

        let source = match &event.source {
            Some(s) => s,
            None => continue,
        };

        let user_id = match &source.user_id {
            Some(uid) => uid.clone(),
            None => continue,
        };

        let reply_token = match &event.reply_token {
            Some(rt) => rt.clone(),
            None => continue,
        };

        info!("LINE message from {}: {}", user_id, text);

        // Route to agent
        match LineState::get_or_create_agent(
            &state.agents,
            &state.data_dir,
            &state.provider_api_key,
            &state.provider_base_url,
            &state.model,
            &user_id,
        )
        .await
        {
            Ok(sender) => {
                if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                    error!("Failed to send to LINE agent {}: {}", user_id, e);
                    state.agents.lock().await.remove(&user_id);
                }
            }
            Err(e) => {
                error!("Cannot create LINE agent for {}: {}", user_id, e);
            }
        }

        // Push reply token to response sender
        let _ = state
            .response_tx
            .send((reply_token, user_id, String::new()))
            .await;
    }

    "ok"
}

// ── Public entry point ──

/// Run the LINE gateway (webhook server).
pub async fn run(
    channel_secret: String,
    channel_access_token: String,
    webhook_port: u16,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    info!("LINE gateway starting on port {}...", webhook_port);

    let (response_tx, mut response_rx) =
        tokio::sync::mpsc::channel::<(String, String, String)>(128);

    let config = Config::from_env()?;
    let state = Arc::new(LineState {
        agents: Mutex::new(HashMap::new()),
        response_tx: response_tx.clone(),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        channel_access_token: channel_access_token.clone(),
    });

    // Spawn response sender
    let cat = channel_access_token.clone();
    tokio::spawn(async move {
        while let Some((reply_token, _user_id, _text)) = response_rx.recv().await {
            // Response text will be populated from agent output
            // For now, send placeholder
            if let Err(e) =
                send_line_reply(&cat, &reply_token, "I'll get back to you shortly!").await
            {
                error!("Failed to send LINE reply: {}", e);
            }
        }
    });

    let app = Router::new()
        .route("/line", post(line_webhook))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", webhook_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("LINE webhook listening on {}/line", addr);

    let _channel_secret = channel_secret; // kept for signature verification in future

    axum::serve(listener, app).await?;

    Ok(())
}
