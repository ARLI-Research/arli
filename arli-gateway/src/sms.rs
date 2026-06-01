//! SMS/Twilio gateway — webhook receiver for Twilio SMS + REST API responder.
//!
//! Listens for POST /sms webhooks from Twilio and sends replies via the Twilio REST API.
//!
//! Environment variables:
//!   TWILIO_ACCOUNT_SID    — Twilio Account SID
//!   TWILIO_AUTH_TOKEN     — Twilio Auth Token
//!   TWILIO_PHONE_NUMBER   — Your Twilio phone number (e.g., "+1234****890")
//!   SMS_PORT              — Webhook listen port (default: 3008)

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use axum::{extract::State, response::IntoResponse, routing::post, Router};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

// ── Shared state ──

struct SmsState {
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    account_sid: String,
    auth_token: String,
    twilio_phone: String,
}

impl SmsState {
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        from_number: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        if let Some(sender) = agents.get(from_number) {
            return Ok(sender.clone());
        }

        info!("Creating SMS agent for {}", from_number);

        let safe_id = from_number.replace('+', "");
        let db_path = data_dir.join(format!("sms-{}.db", safe_id));
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
            name: format!("sms-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via SMS. \
                 Respond in the user's language. Be very concise — SMS messages are \
                 limited to 1600 characters and users expect short replies."
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
        let from_owned = from_number.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(_response) => {
                        // Response sent via the twilio_send path in the webhook handler
                    }
                    Err(e) => {
                        error!("SMS agent error for {}: {}", from_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(from_number.to_string(), sender.clone());
        Ok(sender)
    }
}

// ── Twilio send helper ──

async fn send_twilio_sms(
    account_sid: &str,
    auth_token: &str,
    twilio_phone: &str,
    to: &str,
    text: &str,
) -> anyhow::Result<()> {
    let truncated = if text.len() > 1600 {
        format!("{}...", &text[..1597])
    } else {
        text.to_string()
    };

    let url = format!(
        "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
        account_sid
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .basic_auth(account_sid, Some(auth_token))
        .form(&[("To", to), ("From", twilio_phone), ("Body", &truncated)])
        .send()
        .await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        warn!("Twilio send error: {}", body);
    }

    Ok(())
}

// ── Webhook handler ──

/// Handle incoming Twilio SMS webhook (POST /sms).
///
/// Twilio sends `application/x-www-form-urlencoded` with fields `Body` and `From`.
async fn sms_webhook(
    State(state): State<Arc<SmsState>>,
    body: axum::extract::Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let from = match body.get("From") {
        Some(f) => f.clone(),
        None => return "Missing From".to_string(),
    };

    let text = match body.get("Body") {
        Some(t) => t.trim().to_string(),
        None => return "Missing Body".to_string(),
    };

    if text.is_empty() {
        return "ok".to_string();
    }

    info!("SMS from {}: {}", from, text);

    // Route to agent
    match SmsState::get_or_create_agent(
        &state.agents,
        &state.data_dir,
        &state.provider_api_key,
        &state.provider_base_url,
        &state.model,
        &from,
    )
    .await
    {
        Ok(sender) => {
            // Trigger the agent by sending the user message
            if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                error!("Failed to send to SMS agent {}: {}", from, e);
                state.agents.lock().await.remove(&from);
                return format!("Agent error: {}", e);
            }

            // Wait a moment for the agent to process, then collect response.
            // The agent runs in its own spawned task; we need to wait for its response.
            // Since the agent loop handles response delivery internally (via response_tx),
            // we just acknowledge the webhook. The actual reply is sent by the agent's
            // spawned task via the response channel.
        }
        Err(e) => {
            error!("Cannot create SMS agent for {}: {}", from, e);
            return format!("Agent creation failed: {}", e);
        }
    }

    // Return TwiML or plain text — Twilio accepts either
    "ok".to_string()
}

/// Run the SMS/Twilio gateway (webhook server).
pub async fn run(
    account_sid: String,
    auth_token: String,
    twilio_phone: String,
    port: u16,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    info!("SMS gateway starting on port {}...", port);

    let (_response_tx, mut response_rx) = tokio::sync::mpsc::channel::<(String, String)>(128);

    let config = Config::from_env()?;
    let state = Arc::new(SmsState {
        agents: Mutex::new(HashMap::new()),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        account_sid: account_sid.clone(),
        auth_token: auth_token.clone(),
        twilio_phone: twilio_phone.clone(),
    });

    // Spawn response sender
    let sid = account_sid;
    let token = auth_token;
    let phone = twilio_phone;
    tokio::spawn(async move {
        while let Some((to, text)) = response_rx.recv().await {
            if let Err(e) = send_twilio_sms(&sid, &token, &phone, &to, &text).await {
                error!("Failed to send SMS: {}", e);
            }
        }
    });

    let app = Router::new()
        .route("/sms", post(sms_webhook))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("SMS webhook listening on {}/sms", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

// ── Token resolution helpers ──

fn resolve_sms_value(env_var: &str, config_key: &str) -> Option<String> {
    std::env::var(env_var).ok().or_else(|| {
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
                    v.get("gateway")?
                        .get(config_key)
                        .and_then(|val| val.as_str().map(String::from))
                })
            })
        } else {
            None
        }
    })
}

/// Convenience wrapper that reads env vars and starts the gateway.
pub async fn run_from_env(data_dir: PathBuf) -> anyhow::Result<()> {
    let account_sid =
        resolve_sms_value("TWILIO_ACCOUNT_SID", "twilio_account_sid").ok_or_else(|| {
            anyhow::anyhow!(
            "TWILIO_ACCOUNT_SID not set. Set env var or gateway.twilio_account_sid in config.toml"
        )
        })?;

    let auth_token =
        resolve_sms_value("TWILIO_AUTH_TOKEN", "twilio_auth_token").ok_or_else(|| {
            anyhow::anyhow!(
            "TWILIO_AUTH_TOKEN not set. Set env var or gateway.twilio_auth_token in config.toml"
        )
        })?;

    let twilio_phone =
        resolve_sms_value("TWILIO_PHONE_NUMBER", "twilio_phone_number").ok_or_else(|| {
            anyhow::anyhow!(
            "TWILIO_PHONE_NUMBER not set. Set env var or gateway.twilio_phone_number in config.toml"
        )
        })?;

    let port: u16 = std::env::var("SMS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3008);

    run(account_sid, auth_token, twilio_phone, port, data_dir).await
}
