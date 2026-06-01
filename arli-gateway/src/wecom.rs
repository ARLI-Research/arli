//! WeCom / Enterprise WeChat gateway — webhook receiver + message sender.
//!
//! Uses WeCom (Enterprise WeChat) APIs (qyapi.weixin.qq.com).
//!
//! Environment variables:
//!   WECOM_CORP_ID         — WeCom Corp ID
//!   WECOM_AGENT_SECRET    — WeCom agent secret
//!   WECOM_TOKEN           — Webhook verification token
//!   WECOM_ENCODING_AES_KEY — Message encryption key
//!   WECOM_PORT            — Webhook listen port (default: 3013)
//!
//! Reference: https://developer.work.weixin.qq.com/document/

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

// ── WeCom API types ──

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

/// Webhook URL verification query params (GET).
#[derive(Debug, Deserialize)]
struct VerifyQuery {
    msg_signature: Option<String>,
    timestamp: Option<String>,
    nonce: Option<String>,
    echostr: Option<String>,
}

/// Inbound message event from WeCom (XML body, parsed via serde).
#[derive(Debug, Deserialize)]
struct WeComXmlMessage {
    #[serde(rename = "ToUserName")]
    to_user_name: Option<String>,
    #[serde(rename = "FromUserName")]
    from_user_name: Option<String>,
    #[serde(rename = "CreateTime")]
    #[allow(dead_code)]
    create_time: Option<String>,
    #[serde(rename = "MsgType")]
    msg_type: Option<String>,
    #[serde(rename = "Content")]
    content: Option<String>,
    #[serde(rename = "MsgId")]
    #[allow(dead_code)]
    msg_id: Option<String>,
    #[serde(rename = "AgentID")]
    #[allow(dead_code)]
    agent_id: Option<String>,
}

/// Outbound text message.
#[derive(Debug, Serialize)]
struct SendMessageRequest {
    touser: String,
    msgtype: String,
    agentid: Option<String>,
    text: WeComTextBody,
}

#[derive(Debug, Serialize)]
struct WeComTextBody {
    content: String,
}

/// Shared state for WeCom webhook handler.
struct WeComState {
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    response_tx: tokio::sync::mpsc::Sender<(String, String)>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    corp_id: String,
    agent_secret: String,
    token: String,
    _encoding_aes_key: String,
}

impl WeComState {
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        user_id: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        let safe_id = user_id.replace([':', '/', '\\', ' ', '@', '|'], "_");

        if let Some(sender) = agents.get(user_id) {
            return Ok(sender.clone());
        }

        info!("Creating WeCom agent for user {}", user_id);

        let db_path = data_dir.join(format!("wecom-{}.db", safe_id));
        let store = SessionStore::open(db_path.clone())?;
        let memory_path = data_dir.join("memory.db");
        let memory_store = Arc::new(MemoryStore::open(memory_path)?);

        let provider = Box::new(OpenAIProvider::new(
            provider_api_key.to_string(),
            model.to_string(),
            provider_base_url.clone(),
        ));

        let mut tools = ToolRegistry::new();
        register_builtin_tools(&mut tools, Some(db_path), Some(memory_store), None, None, None);

        let agent_config = AgentConfig {
            name: format!("wecom-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via WeCom (Enterprise WeChat). \
                 Respond in the user's language. Be concise and use plain text \
                 or basic formatting."
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
                        error!("WeCom agent {} error: {}", user_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(user_id.to_string(), sender.clone());
        Ok(sender)
    }
}

// ── WeCom API helpers ──

/// Obtain an access token from WeCom.
async fn get_access_token(corp_id: &str, agent_secret: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://qyapi.weixin.qq.com/cgi-bin/gettoken?corpid={}&corpsecret={}",
        corp_id, agent_secret
    );

    let resp: TokenResponse = client.get(&url).send().await?.json().await?;

    resp.access_token
        .ok_or_else(|| anyhow::anyhow!("WeCom token response missing access_token"))
}

/// Send a text message via WeCom API.
async fn send_wecom_message(
    corp_id: &str,
    agent_secret: &str,
    to_user: &str,
    text: &str,
) -> anyhow::Result<()> {
    let token = get_access_token(corp_id, agent_secret).await?;

    let truncated = if text.len() > 2000 {
        format!("{}...", &text[..1997])
    } else {
        text.to_string()
    };

    let req = SendMessageRequest {
        touser: to_user.to_string(),
        msgtype: "text".to_string(),
        agentid: None, // Will use default agent
        text: WeComTextBody { content: truncated },
    };

    let client = reqwest::Client::new();
    let url = format!(
        "https://qyapi.weixin.qq.com/cgi-bin/message/send?access_token={}",
        token
    );
    let resp = client.post(&url).json(&req).send().await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        warn!("WeCom send error ({}): {}", status, body);
    }

    Ok(())
}

// ── Webhook handlers ──

/// URL verification (GET).
async fn wecom_verify(
    Query(params): Query<VerifyQuery>,
    State(_state): State<Arc<WeComState>>,
) -> impl IntoResponse {
    let echostr = params.echostr.as_deref().unwrap_or("");

    // In production, verify the msg_signature using the token and encoding AES key.
    // For now, echo back the challenge string.
    info!("WeCom webhook URL verification");

    echostr.to_string()
}

/// Receive messages (POST).
async fn wecom_webhook(State(state): State<Arc<WeComState>>, body: String) -> impl IntoResponse {
    // WeCom sends XML payloads. Parse as XML via serde.
    // Strip any leading BOM or whitespace.
    let body = body.trim();

    // Try to parse as XML
    let msg: WeComXmlMessage = match quick_xml::de::from_str(body) {
        Ok(m) => m,
        Err(e) => {
            warn!("Failed to parse WeCom XML: {}", e);
            return "success".to_string();
        }
    };

    let msg_type = msg.msg_type.as_deref().unwrap_or("");

    if msg_type != "text" {
        return "success".to_string();
    }

    let text = match &msg.content {
        Some(t) => t.trim().to_string(),
        None => return "success".to_string(),
    };

    if text.is_empty() {
        return "success".to_string();
    }

    let user_id = match &msg.from_user_name {
        Some(uid) => uid.clone(),
        None => return "success".to_string(),
    };

    info!("WeCom message from {}: {}", user_id, text);

    match WeComState::get_or_create_agent(
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
                error!("Failed to send to WeCom agent {}: {}", user_id, e);
                state.agents.lock().await.remove(&user_id);
            }
        }
        Err(e) => {
            error!("Cannot create WeCom agent for {}: {}", user_id, e);
        }
    }

    "success".to_string()
}

// ── Public entry point ──

/// Run the WeCom gateway (webhook server).
pub async fn run(
    corp_id: String,
    agent_secret: String,
    token: String,
    encoding_aes_key: String,
    webhook_port: u16,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    info!("WeCom gateway starting on port {}...", webhook_port);

    let (response_tx, mut response_rx) = tokio::sync::mpsc::channel::<(String, String)>(128);

    let config = Config::from_env()?;
    let state = Arc::new(WeComState {
        agents: Mutex::new(HashMap::new()),
        response_tx: response_tx.clone(),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        corp_id: corp_id.clone(),
        agent_secret: agent_secret.clone(),
        token: token.clone(),
        _encoding_aes_key: encoding_aes_key.clone(),
    });

    // Spawn response sender
    let cid = corp_id.clone();
    let asec = agent_secret.clone();
    tokio::spawn(async move {
        while let Some((user_id, text)) = response_rx.recv().await {
            if let Err(e) = send_wecom_message(&cid, &asec, &user_id, &text).await {
                error!("Failed to send WeCom message: {}", e);
            }
        }
    });

    let app = Router::new()
        .route("/wecom", get(wecom_verify).post(wecom_webhook))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", webhook_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("WeCom webhook listening on {}/wecom", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
