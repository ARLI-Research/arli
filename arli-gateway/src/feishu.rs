//! Feishu / Lark gateway — webhook receiver + message sender.
//!
//! Uses Feishu Open Platform APIs (open.feishu.cn).
//!
//! Environment variables:
//!   FEISHU_APP_ID             — Feishu app ID
//!   FEISHU_APP_SECRET         — Feishu app secret
//!   FEISHU_VERIFICATION_TOKEN — Webhook verification token
//!   FEISHU_PORT               — Webhook listen port (default: 3010)
//!
//! Reference: https://open.feishu.cn/document/

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

// ── Feishu API types ──

/// Tenant access token response.
#[derive(Debug, Deserialize)]
struct TenantTokenResponse {
    #[allow(dead_code)]
    code: i32,
    #[allow(dead_code)]
    msg: Option<String>,
    tenant_access_token: Option<String>,
    #[allow(dead_code)]
    expire: Option<i64>,
}

/// Inbound event from Feishu webhook.
#[derive(Debug, Deserialize)]
struct FeishuEvent {
    schema: Option<String>,
    header: Option<FeishuEventHeader>,
    event: Option<FeishuEventBody>,
    challenge: Option<String>,
    token: Option<String>,
    #[serde(rename = "type")]
    event_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeishuEventHeader {
    event_type: Option<String>,
    token: Option<String>,
    #[allow(dead_code)]
    event_id: Option<String>,
    #[allow(dead_code)]
    create_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeishuEventBody {
    sender: Option<FeishuSender>,
    message: Option<FeishuMessageContent>,
}

#[derive(Debug, Deserialize)]
struct FeishuSender {
    sender_id: Option<FeishuSenderId>,
}

#[derive(Debug, Deserialize)]
struct FeishuSenderId {
    open_id: Option<String>,
    #[allow(dead_code)]
    user_id: Option<String>,
    #[allow(dead_code)]
    union_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeishuMessageContent {
    message_id: Option<String>,
    chat_id: Option<String>,
    chat_type: Option<String>,
    content: Option<String>, // JSON-encoded string
    #[allow(dead_code)]
    msg_type: Option<String>,
}

/// Parsed text content from message.
#[derive(Debug, Deserialize)]
struct FeishuTextContent {
    text: Option<String>,
}

/// Outbound message request.
#[derive(Debug, Serialize)]
struct SendMessageRequest {
    receive_id: String,
    msg_type: String,
    content: String,
}

/// Challenge response for URL verification.
#[derive(Debug, Serialize)]
struct ChallengeResponse {
    challenge: String,
}

/// Shared state for Feishu webhook handler.
struct FeishuState {
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    response_tx: tokio::sync::mpsc::Sender<(String, String)>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    app_id: String,
    app_secret: String,
    verification_token: String,
}

impl FeishuState {
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        chat_id: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        let safe_id = chat_id.replace([':', '/', '\\', ' ', '@'], "_");

        if let Some(sender) = agents.get(chat_id) {
            return Ok(sender.clone());
        }

        info!("Creating Feishu agent for chat {}", chat_id);

        let db_path = data_dir.join(format!("feishu-{}.db", safe_id));
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
            name: format!("feishu-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via Feishu/Lark. \
                 Respond in the user's language. Be concise and use plain text \
                 or basic markdown formatting."
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

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(_response) => {
                        // Response handled via response channel
                    }
                    Err(e) => {
                        error!("Feishu agent {} error: {}", chat_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(chat_id.to_string(), sender.clone());
        Ok(sender)
    }
}

// ── Feishu API helpers ──

/// Obtain a tenant access token from Feishu.
async fn get_tenant_token(app_id: &str, app_secret: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "app_id": app_id,
        "app_secret": app_secret,
    });

    let resp: TenantTokenResponse = client
        .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    resp.tenant_access_token
        .ok_or_else(|| anyhow::anyhow!("Feishu token response missing tenant_access_token"))
}

/// Send a text message to a Feishu chat.
async fn send_feishu_message(
    app_id: &str,
    app_secret: &str,
    chat_id: &str,
    text: &str,
) -> anyhow::Result<()> {
    let token = get_tenant_token(app_id, app_secret).await?;

    let truncated = if text.len() > 4000 {
        format!("{}...", &text[..3997])
    } else {
        text.to_string()
    };

    let content = serde_json::json!({
        "text": truncated,
    });

    let req = SendMessageRequest {
        receive_id: chat_id.to_string(),
        msg_type: "text".to_string(),
        content: content.to_string(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post("https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=chat_id")
        .header("Authorization", format!("Bearer {}", token))
        .json(&req)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        warn!("Feishu send error ({}): {}", status, body);
    }

    Ok(())
}

// ── Webhook handler ──

async fn feishu_webhook(
    State(state): State<Arc<FeishuState>>,
    Json(event): Json<FeishuEvent>,
) -> impl IntoResponse {
    // URL verification challenge
    if let Some(challenge) = &event.challenge {
        if let Some(token) = &event.token {
            if token == &state.verification_token {
                info!("Feishu webhook URL verified");
                return Json(ChallengeResponse {
                    challenge: challenge.clone(),
                })
                .into_response();
            }
        }
        return Json(serde_json::json!({"challenge": ""})).into_response();
    }

    // Handle message events
    let header = match &event.header {
        Some(h) => h,
        None => return Json(serde_json::json!({"code": 0})).into_response(),
    };

    let event_type = header.event_type.as_deref().unwrap_or("");

    if event_type != "im.message.receive_v1" {
        return Json(serde_json::json!({"code": 0})).into_response();
    }

    let body = match &event.event {
        Some(b) => b,
        None => return Json(serde_json::json!({"code": 0})).into_response(),
    };

    let message = match &body.message {
        Some(m) => m,
        None => return Json(serde_json::json!({"code": 0})).into_response(),
    };

    let chat_id = match &message.chat_id {
        Some(id) => id.clone(),
        None => return Json(serde_json::json!({"code": 0})).into_response(),
    };

    // Parse text from JSON-encoded content field
    let text = match &message.content {
        Some(content_str) => match serde_json::from_str::<FeishuTextContent>(content_str) {
            Ok(tc) => tc.text.unwrap_or_default().trim().to_string(),
            Err(_) => return Json(serde_json::json!({"code": 0})).into_response(),
        },
        None => return Json(serde_json::json!({"code": 0})).into_response(),
    };

    if text.is_empty() {
        return Json(serde_json::json!({"code": 0})).into_response();
    }

    let user_id = body
        .sender
        .as_ref()
        .and_then(|s| s.sender_id.as_ref())
        .and_then(|id| id.open_id.clone())
        .unwrap_or_else(|| "unknown".to_string());

    info!(
        "Feishu message from {} (chat {}): {}",
        user_id, chat_id, text
    );

    match FeishuState::get_or_create_agent(
        &state.agents,
        &state.data_dir,
        &state.provider_api_key,
        &state.provider_base_url,
        &state.model,
        &chat_id,
    )
    .await
    {
        Ok(sender) => {
            if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                error!("Failed to send to Feishu agent {}: {}", chat_id, e);
                state.agents.lock().await.remove(&chat_id);
            }
        }
        Err(e) => {
            error!("Cannot create Feishu agent for {}: {}", chat_id, e);
        }
    }

    Json(serde_json::json!({"code": 0})).into_response()
}

// ── Public entry point ──

/// Run the Feishu gateway (webhook server).
pub async fn run(
    app_id: String,
    app_secret: String,
    verification_token: String,
    webhook_port: u16,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    info!("Feishu gateway starting on port {}...", webhook_port);

    let (response_tx, mut response_rx) = tokio::sync::mpsc::channel::<(String, String)>(128);

    let config = Config::from_env()?;
    let state = Arc::new(FeishuState {
        agents: Mutex::new(HashMap::new()),
        response_tx: response_tx.clone(),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        app_id: app_id.clone(),
        app_secret: app_secret.clone(),
        verification_token: verification_token.clone(),
    });

    // Spawn response sender
    let aid = app_id.clone();
    let asec = app_secret.clone();
    tokio::spawn(async move {
        while let Some((chat_id, text)) = response_rx.recv().await {
            if let Err(e) = send_feishu_message(&aid, &asec, &chat_id, &text).await {
                error!("Failed to send Feishu message: {}", e);
            }
        }
    });

    let app = Router::new()
        .route("/feishu", post(feishu_webhook))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", webhook_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Feishu webhook listening on {}/feishu", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
