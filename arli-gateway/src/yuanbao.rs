//! Yuanbao gateway — Tencent Yuanbao webhook receiver + API responder.
//!
//! Receives callbacks at POST /yuanbao and replies via Yuanbao REST API.
//!
//! Environment variables:
//!   YUANBAO_APP_ID     — Yuanbao app ID
//!   YUANBAO_APP_SECRET — Yuanbao app secret
//!   YUANBAO_PORT       — Webhook port (default: 3014)
//!
//! Reference: https://yuanbao.tencent.com/

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

// ── Yuanbao webhook types ──

#[derive(Debug, Deserialize)]
struct YuanbaoWebhook {
    #[serde(rename = "msgType")]
    msg_type: Option<String>,
    #[serde(rename = "userId")]
    user_id: Option<String>,
    #[serde(rename = "groupId")]
    group_id: Option<String>,
    content: Option<YuanbaoContent>,
    #[serde(rename = "msgId")]
    msg_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct YuanbaoContent {
    text: Option<String>,
}

#[derive(Debug, Serialize)]
struct YuanbaoReply {
    #[serde(rename = "msgType")]
    msg_type: String,
    content: YuanbaoTextContent,
    #[serde(rename = "replyTo")]
    reply_to: Option<String>,
}

#[derive(Debug, Serialize)]
struct YuanbaoTextContent {
    text: String,
}

/// Shared state for Yuanbao webhook handler.
struct YuanbaoState {
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    app_id: String,
    app_secret: String,
}

impl YuanbaoState {
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        chat_id: &str,
        app_id: &str,
        app_secret: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        let safe_id = chat_id.replace([':', '/', ' ', '@', '\\'], "_");

        if let Some(sender) = agents.get(chat_id) {
            return Ok(sender.clone());
        }

        info!("Creating Yuanbao agent for chat {}", chat_id);

        let db_path = data_dir.join(format!("yuanbao-{}.db", safe_id));
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
            name: format!("yuanbao-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via Yuanbao. \
                 Respond in the user's language. Be concise.".to_string()
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
        let secret_owned = app_secret.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        if let Err(e) = send_yuanbao_reply(
                            &app_owned,
                            &secret_owned,
                            &chat_owned,
                            &response,
                            None,
                        ).await
                        {
                            error!("Failed to send Yuanbao reply: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Yuanbao agent error for chat {}: {}", chat_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(chat_id.to_string(), sender.clone());
        Ok(sender)
    }
}

/// Send a reply via Yuanbao API.
async fn send_yuanbao_reply(
    app_id: &str,
    app_secret: &str,
    chat_id: &str,
    text: &str,
    reply_to: Option<&str>,
) -> anyhow::Result<()> {
    let truncated = if text.len() > 2000 {
        format!("{}... _(truncated)_", &text[..1990])
    } else {
        text.to_string()
    };

    let url = format!("https://open.yuanbao.tencent.com/api/v1/chat/{}/messages", chat_id);

    let body = YuanbaoReply {
        msg_type: "text".to_string(),
        content: YuanbaoTextContent {
            text: truncated,
        },
        reply_to: reply_to.map(|s| s.to_string()),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("X-App-Id", app_id)
        .header("X-App-Secret", app_secret)
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        warn!("Yuanbao send error ({}): {}", status, resp.text().await.unwrap_or_default());
    }

    Ok(())
}

/// Webhook handler for incoming Yuanbao callbacks (POST /yuanbao).
async fn yuanbao_webhook(
    State(state): State<Arc<YuanbaoState>>,
    Json(payload): Json<YuanbaoWebhook>,
) -> impl IntoResponse {
    let msg_type = payload.msg_type.as_deref().unwrap_or("");
    if msg_type != "text" {
        return "ok";
    }

    let text = match &payload.content {
        Some(c) => c.text.as_deref().unwrap_or("").trim().to_string(),
        None => return "ok",
    };

    if text.is_empty() {
        return "ok";
    }

    let user_id = payload.user_id.as_deref().unwrap_or("unknown");
    let chat_id = payload.group_id.as_deref().unwrap_or(user_id);
    let _reply_to = payload.msg_id.as_deref();

    info!("Yuanbao message from {} (chat {}): {}", user_id, chat_id, text);

    match YuanbaoState::get_or_create_agent(
        &state.agents,
        &state.data_dir,
        &state.provider_api_key,
        &state.provider_base_url,
        &state.model,
        chat_id,
        &state.app_id,
        &state.app_secret,
    ).await
    {
        Ok(sender) => {
            if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                error!("Failed to send to Yuanbao agent {}: {}", chat_id, e);
                state.agents.lock().await.remove(chat_id);
            }
        }
        Err(e) => {
            error!("Cannot create Yuanbao agent for {}: {}", chat_id, e);
        }
    }

    "ok"
}

/// Run the Yuanbao gateway (webhook server).
pub async fn run(
    app_id: String,
    app_secret: String,
    webhook_port: u16,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    info!("Yuanbao gateway starting on port {}...", webhook_port);

    let config = Config::from_env()?;
    let state = Arc::new(YuanbaoState {
        agents: Mutex::new(HashMap::new()),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        app_id,
        app_secret,
    });

    let app = Router::new()
        .route("/yuanbao", post(yuanbao_webhook))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", webhook_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Yuanbao webhook listening on {}/yuanbao", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
