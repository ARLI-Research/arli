//! DingTalk gateway — webhook receiver + message sender.
//!
//! Uses DingTalk Open Platform APIs (oapi.dingtalk.com).
//!
//! Environment variables:
//!   DINGTALK_APP_KEY    — DingTalk app key (AppKey)
//!   DINGTALK_APP_SECRET — DingTalk app secret (AppSecret)
//!   DINGTALK_PORT       — Webhook listen port (default: 3011)
//!
//! Reference: https://open.dingtalk.com/document/

use arli_core::{
    Agent, AgentConfig, AgentMessage, Config,
    OpenAIProvider, SessionStore, ToolRegistry,
    memory::MemoryStore,
};
use arli_core::tools::builtin::register_builtin_tools;
use axum::{
    extract::State,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn, error};

// ── DingTalk API types ──

/// Access token response.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[allow(dead_code)]
    errcode: i32,
    #[allow(dead_code)]
    errmsg: Option<String>,
    access_token: Option<String>,
    #[allow(dead_code)]
    expires_in: Option<i64>,
}

/// Inbound webhook event from DingTalk.
#[derive(Debug, Deserialize)]
struct DingTalkEvent {
    #[serde(rename = "conversationId")]
    conversation_id: Option<String>,
    #[serde(rename = "conversationType")]
    #[allow(dead_code)]
    conversation_type: Option<String>,
    #[serde(rename = "msgtype")]
    msg_type: Option<String>,
    text: Option<DingTalkText>,
    #[serde(rename = "senderId")]
    sender_id: Option<String>,
    #[serde(rename = "senderNick")]
    sender_nick: Option<String>,
    #[serde(rename = "senderCorpId")]
    #[allow(dead_code)]
    sender_corp_id: Option<String>,
    #[serde(rename = "sessionWebhook")]
    session_webhook: Option<String>,
    #[serde(rename = "chatbotUserId")]
    #[allow(dead_code)]
    chatbot_user_id: Option<String>,
    #[serde(rename = "isAdmin")]
    #[allow(dead_code)]
    is_admin: Option<bool>,
    #[serde(rename = "createAt")]
    #[allow(dead_code)]
    create_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DingTalkText {
    content: Option<String>,
}

/// Outbound message to DingTalk robot API.
#[derive(Debug, Serialize)]
struct SendMessageRequest {
    msgtype: String,
    text: DingTalkTextBody,
}

#[derive(Debug, Serialize)]
struct DingTalkTextBody {
    content: String,
}

/// Shared state for DingTalk webhook handler.
struct DingTalkState {
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    response_tx: tokio::sync::mpsc::Sender<(String, String)>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    app_key: String,
    app_secret: String,
}

impl DingTalkState {
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        conversation_id: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        let safe_id = conversation_id.replace([':', '/', '\\', ' ', '@'], "_");

        if let Some(sender) = agents.get(conversation_id) {
            return Ok(sender.clone());
        }

        info!("Creating DingTalk agent for conversation {}", conversation_id);

        let db_path = data_dir.join(format!("dingtalk-{}.db", safe_id));
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
            name: format!("dingtalk-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via DingTalk. \
                 Respond in the user's language. Be concise and use plain text.".to_string()
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
        let conv_owned = conversation_id.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(_response) => {
                        // Response handled via response channel
                    }
                    Err(e) => {
                        error!("DingTalk agent {} error: {}", conv_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(conversation_id.to_string(), sender.clone());
        Ok(sender)
    }
}

// ── DingTalk API helpers ──

/// Obtain an access token from DingTalk.
async fn get_access_token(app_key: &str, app_secret: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://oapi.dingtalk.com/gettoken?appkey={}&appsecret={}",
        app_key, app_secret
    );

    let resp: TokenResponse = client
        .get(&url)
        .send()
        .await?
        .json()
        .await?;

    resp.access_token
        .ok_or_else(|| anyhow::anyhow!("DingTalk token response missing access_token"))
}

/// Send a text message via DingTalk robot API.
async fn send_dingtalk_message(
    app_key: &str,
    app_secret: &str,
    session_webhook: &str,
    text: &str,
) -> anyhow::Result<()> {
    // DingTalk robot messages can be sent via session webhook directly,
    // or via the API with an access token. Use session webhook when available.
    if !session_webhook.is_empty() {
        let truncated = if text.len() > 20000 {
            format!("{}...", &text[..19997])
        } else {
            text.to_string()
        };

        let req = SendMessageRequest {
            msgtype: "text".to_string(),
            text: DingTalkTextBody {
                content: truncated,
            },
        };

        let client = reqwest::Client::new();
        let resp = client
            .post(session_webhook)
            .json(&req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!("DingTalk send error ({}): {}", status, body);
        }
    } else {
        // Fall back to API-based send
        let token = get_access_token(app_key, app_secret).await?;

        let truncated = if text.len() > 20000 {
            format!("{}...", &text[..19997])
        } else {
            text.to_string()
        };

        let req = SendMessageRequest {
            msgtype: "text".to_string(),
            text: DingTalkTextBody {
                content: truncated,
            },
        };

        let client = reqwest::Client::new();
        let url = format!(
            "https://oapi.dingtalk.com/robot/send?access_token={}",
            token
        );
        let resp = client
            .post(&url)
            .json(&req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!("DingTalk send error ({}): {}", status, body);
        }
    }

    Ok(())
}

// ── Webhook handler ──

async fn dingtalk_webhook(
    State(state): State<Arc<DingTalkState>>,
    Json(event): Json<DingTalkEvent>,
) -> impl IntoResponse {
    let conversation_id = match &event.conversation_id {
        Some(id) => id.clone(),
        None => return Json(serde_json::json!({"errcode": 0})).into_response(),
    };

    let text = match &event.text {
        Some(t) => t.content.as_deref().unwrap_or("").trim().to_string(),
        None => return Json(serde_json::json!({"errcode": 0})).into_response(),
    };

    if text.is_empty() {
        return Json(serde_json::json!({"errcode": 0})).into_response();
    }

    let sender_id = event.sender_id.as_deref().unwrap_or("unknown");
    let sender_nick = event.sender_nick.as_deref().unwrap_or(sender_id);

    info!(
        "DingTalk message from {} (conv {}): {}",
        sender_nick, conversation_id, text
    );

    match DingTalkState::get_or_create_agent(
        &state.agents,
        &state.data_dir,
        &state.provider_api_key,
        &state.provider_base_url,
        &state.model,
        &conversation_id,
    )
    .await
    {
        Ok(sender) => {
            if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                error!(
                    "Failed to send to DingTalk agent {}: {}",
                    conversation_id, e
                );
                state.agents.lock().await.remove(&conversation_id);
            }
        }
        Err(e) => {
            error!(
                "Cannot create DingTalk agent for {}: {}",
                conversation_id, e
            );
        }
    }

    Json(serde_json::json!({"errcode": 0})).into_response()
}

// ── Public entry point ──

/// Run the DingTalk gateway (webhook server).
pub async fn run(
    app_key: String,
    app_secret: String,
    webhook_port: u16,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    info!("DingTalk gateway starting on port {}...", webhook_port);

    let (response_tx, mut response_rx) =
        tokio::sync::mpsc::channel::<(String, String)>(128);

    let config = Config::from_env()?;
    let state = Arc::new(DingTalkState {
        agents: Mutex::new(HashMap::new()),
        response_tx: response_tx.clone(),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        app_key: app_key.clone(),
        app_secret: app_secret.clone(),
    });

    // Spawn response sender
    let ak = app_key.clone();
    let asec = app_secret.clone();
    tokio::spawn(async move {
        while let Some((conversation_id, text)) = response_rx.recv().await {
            if let Err(e) = send_dingtalk_message(&ak, &asec, "", &text).await {
                error!("Failed to send DingTalk message: {}", e);
            }
            let _ = conversation_id; // unused in session-webhook path
        }
    });

    let app = Router::new()
        .route("/dingtalk", post(dingtalk_webhook))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", webhook_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("DingTalk webhook listening on {}/dingtalk", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
