//! WhatsApp gateway — Cloud API webhook receiver + message sender.
//!
//! Uses Meta's WhatsApp Cloud API (graph.facebook.com).
//! Requires: phone_number_id, access_token, verify_token, webhook_port.
//!
//! Reference: https://developers.facebook.com/docs/whatsapp/cloud-api

use arli_core::{
    Agent, AgentConfig, AgentMessage, Config,
    OpenAIProvider, SessionStore, ToolRegistry,
    memory::MemoryStore,
};
use arli_core::tools::builtin::register_builtin_tools;
use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn, error};

// ── WhatsApp Cloud API types ──

#[derive(Debug, Deserialize)]
struct WebhookQuery {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebhookBody {
    object: Option<String>,
    entry: Option<Vec<WebhookEntry>>,
}

#[derive(Debug, Deserialize)]
struct WebhookEntry {
    changes: Option<Vec<WebhookChange>>,
}

#[derive(Debug, Deserialize)]
struct WebhookChange {
    value: Option<WebhookValue>,
}

#[derive(Debug, Deserialize)]
struct WebhookValue {
    messages: Option<Vec<WhatsAppMessage>>,
}

#[derive(Debug, Deserialize)]
struct WhatsAppMessage {
    from: Option<String>,
    #[allow(dead_code)]
    id: Option<String>,
    text: Option<WhatsAppText>,
}

#[derive(Debug, Deserialize)]
struct WhatsAppText {
    body: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest {
    messaging_product: String,
    to: String,
    #[serde(rename = "type")]
    msg_type: String,
    text: WhatsAppTextBody,
}

#[derive(Debug, Serialize)]
struct WhatsAppTextBody {
    body: String,
}

/// Shared state for WhatsApp webhook handler.
struct WaState {
    /// Per-phone agent senders.
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    /// Response channel: (to_phone, text).
    #[allow(dead_code)]
    response_tx: tokio::sync::mpsc::Sender<(String, String)>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    /// WhatsApp Cloud API config.
    #[allow(dead_code)]
    phone_number_id: String,
    #[allow(dead_code)]
    access_token: String,
    verify_token: String,
}

impl WaState {
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        phone: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        if let Some(sender) = agents.get(phone) {
            return Ok(sender.clone());
        }

        info!("Creating WhatsApp agent for {}", phone);

        let db_path = data_dir.join(format!("wa-{}.db", phone));
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
            name: format!("wa-{}", phone),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via WhatsApp. \
                 Respond in the user's language. Be concise — WhatsApp messages work best short.".to_string()
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
        let phone_owned = phone.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(_response) => {
                        // Response is handled by the response channel
                    }
                    Err(e) => {
                        error!("WhatsApp agent {} error: {}", phone_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(phone.to_string(), sender.clone());
        Ok(sender)
    }
}

/// Webhook verification (GET).
async fn webhook_verify(
    Query(params): Query<WebhookQuery>,
    State(state): State<Arc<WaState>>,
) -> impl IntoResponse {
    let mode = params.mode.as_deref().unwrap_or("");
    let token = params.verify_token.as_deref().unwrap_or("");
    let challenge = params.challenge.as_deref().unwrap_or("");

    if mode == "subscribe" && token == state.verify_token {
        info!("WhatsApp webhook verified");
        challenge.to_string()
    } else {
        warn!("WhatsApp webhook verification failed");
        "Verification failed".to_string()
    }
}

/// Webhook message handler (POST).
async fn webhook_receive(
    State(state): State<Arc<WaState>>,
    Json(body): Json<WebhookBody>,
) -> impl IntoResponse {
    if body.object.as_deref() != Some("whatsapp_business_account") {
        return "ok";
    }

    let entries = match body.entry {
        Some(e) => e,
        None => return "ok",
    };

    for entry in entries {
        let changes = match entry.changes {
            Some(c) => c,
            None => continue,
        };

        for change in changes {
            let value = match change.value {
                Some(v) => v,
                None => continue,
            };

            let messages = match value.messages {
                Some(m) => m,
                None => continue,
            };

            for msg in messages {
                let from = match &msg.from {
                    Some(f) => f.clone(),
                    None => continue,
                };

                let body = match &msg.text {
                    Some(t) => t.body.as_deref().unwrap_or("").trim().to_string(),
                    None => continue,
                };

                if body.is_empty() {
                    continue;
                }

                info!("WhatsApp message from {}: {}", from, body);

                // Route to agent
                match WaState::get_or_create_agent(
                    &state.agents,
                    &state.data_dir,
                    &state.provider_api_key,
                    &state.provider_base_url,
                    &state.model,
                    &from,
                ).await {
                    Ok(sender) => {
                        if let Err(e) = sender.send(AgentMessage::UserMessage(body)).await {
                            error!("Failed to send to WhatsApp agent {}: {}", from, e);
                            state.agents.lock().await.remove(&from);
                        }
                    }
                    Err(e) => {
                        error!("Cannot create WhatsApp agent for {}: {}", from, e);
                    }
                }
            }
        }
    }

    "ok"
}

/// Send a message via WhatsApp Cloud API.
async fn send_whatsapp_message(
    phone_number_id: &str,
    access_token: &str,
    to: &str,
    text: &str,
) -> anyhow::Result<()> {
    let url = format!(
        "https://graph.facebook.com/v21.0/{}/messages",
        phone_number_id
    );

    let req = SendMessageRequest {
        messaging_product: "whatsapp".to_string(),
        to: to.to_string(),
        msg_type: "text".to_string(),
        text: WhatsAppTextBody {
            body: text.to_string(),
        },
    };

    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&req)
        .send()
        .await?
        .json()
        .await?;

    if let Some(err) = resp.get("error") {
        warn!("WhatsApp send error: {:?}", err);
    }

    Ok(())
}

/// Run the WhatsApp gateway (webhook server).
pub async fn run(
    phone_number_id: String,
    access_token: String,
    verify_token: String,
    webhook_port: u16,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    info!("WhatsApp gateway starting on port {}...", webhook_port);

    let (response_tx, mut response_rx) = tokio::sync::mpsc::channel::<(String, String)>(128);

    let config = Config::from_env()?;
    let state = Arc::new(WaState {
        agents: Mutex::new(HashMap::new()),
        response_tx: response_tx.clone(),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        phone_number_id: phone_number_id.clone(),
        access_token: access_token.clone(),
        verify_token: verify_token.clone(),
    });

    // Spawn response sender
    let pnid = phone_number_id.clone();
    let token = access_token.clone();
    tokio::spawn(async move {
        while let Some((to, text)) = response_rx.recv().await {
            if let Err(e) = send_whatsapp_message(&pnid, &token, &to, &text).await {
                error!("Failed to send WhatsApp message: {}", e);
            }
        }
    });

    // Build axum router
    let app = Router::new()
        .route("/webhook", get(webhook_verify).post(webhook_receive))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", webhook_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("WhatsApp webhook listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
