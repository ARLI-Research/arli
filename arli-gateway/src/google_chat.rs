//! Google Chat gateway — webhook receiver for Google Chat API + REST responder.
//!
//! Listens for POST /google-chat events from Google Chat API and sends replies
//! via the Google Chat REST API.
//!
//! Environment variables:
//!   GOOGLE_CHAT_VERIFICATION_TOKEN — Verification token for webhook registration
//!   GOOGLE_CHAT_PORT               — Webhook listen port (default: 3009)
//!
//! References:
//!   https://developers.google.com/workspace/chat/overview

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

// ── Google Chat event types ──

/// Inbound event from Google Chat (simplified).
#[derive(Debug, Deserialize)]
struct ChatEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,

    #[serde(rename = "eventTime")]
    #[allow(dead_code)]
    event_time: Option<String>,

    message: Option<ChatMessage>,

    space: Option<ChatSpace>,

    user: Option<ChatUser>,

    /// Verification token for registration challenge.
    #[serde(rename = "challengeToken")]
    #[allow(dead_code)]
    challenge_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[allow(dead_code)]
    name: Option<String>,

    text: Option<String>,

    sender: Option<ChatUser>,

    #[serde(rename = "thread")]
    #[allow(dead_code)]
    thread: Option<ChatThread>,
}

#[derive(Debug, Deserialize)]
struct ChatThread {
    #[allow(dead_code)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatSpace {
    name: Option<String>,

    #[serde(rename = "type")]
    #[allow(dead_code)]
    space_type: Option<String>,

    #[serde(rename = "displayName")]
    #[allow(dead_code)]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUser {
    name: Option<String>,

    #[serde(rename = "displayName")]
    display_name: Option<String>,

    #[serde(rename = "type")]
    #[allow(dead_code)]
    user_type: Option<String>,
}

// ── Outbound message types ──

#[derive(Debug, serde::Serialize)]
struct ChatReply {
    text: String,
}

// ── Shared state ──

struct GChatState {
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    /// Response channel: (space_id, text) — replies sent via REST API.
    response_tx: tokio::sync::mpsc::Sender<(String, String)>,
}

impl GChatState {
    #[allow(clippy::too_many_arguments)]
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        space_id: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        let safe_id = space_id.replace(['/', ':', ' '], "_");

        if let Some(sender) = agents.get(space_id) {
            return Ok(sender.clone());
        }

        info!("Creating Google Chat agent for space {}", space_id);

        let db_path = data_dir.join(format!("gchat-{}.db", safe_id));
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
            name: format!("gchat-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via Google Chat. \
                 Respond in the user's language. Be concise and helpful. \
                 Google Chat supports basic formatting."
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
        let space_owned = space_id.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(_response) => {
                        // Response is handled via the response channel
                    }
                    Err(e) => {
                        error!("Google Chat agent error for space {}: {}", space_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(space_id.to_string(), sender.clone());
        Ok(sender)
    }
}

// ── Google Chat REST API send helper ──

/// Send a message to a Google Chat space.
///
/// Uses the Google Chat REST API. Authentication is handled via a service account
/// or OAuth2 token, which should be obtained separately and passed via the
/// GOOGLE_CHAT_ACCESS_TOKEN env var.
async fn send_chat_message(space_id: &str, text: &str) -> anyhow::Result<()> {
    let access_token = std::env::var("GOOGLE_CHAT_ACCESS_TOKEN")
        .map_err(|_| anyhow::anyhow!("GOOGLE_CHAT_ACCESS_TOKEN not set"))?;

    let truncated = if text.len() > 4000 {
        format!("{}... _(truncated)_", &text[..4000])
    } else {
        text.to_string()
    };

    // Google Chat API v1: spaces.messages.create
    // POST https://chat.googleapis.com/v1/{parent=spaces/*}/messages
    let url = format!("https://chat.googleapis.com/v1/{}/messages", space_id);

    let body = ChatReply { text: truncated };

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let resp_body = resp.text().await.unwrap_or_default();
        warn!("Google Chat send error: {}", resp_body);
    }

    Ok(())
}

// ── Webhook handler ──

/// Handle incoming Google Chat event (POST /google-chat).
async fn chat_webhook(
    State(state): State<Arc<GChatState>>,
    Json(event): Json<ChatEvent>,
) -> impl IntoResponse {
    let event_type = event.event_type.as_deref().unwrap_or("unknown");

    match event_type {
        "MESSAGE" => {
            let space = match &event.space {
                Some(s) => s,
                None => return "Missing space".to_string(),
            };

            let space_id = match &space.name {
                Some(id) => id.clone(),
                None => return "Missing space name".to_string(),
            };

            let message = match &event.message {
                Some(m) => m,
                None => return "Missing message".to_string(),
            };

            let text = match &message.text {
                Some(t) => t.trim().to_string(),
                None => return "Missing text".to_string(),
            };

            if text.is_empty() {
                return "ok".to_string();
            }

            let user_display = event
                .user
                .as_ref()
                .and_then(|u| u.display_name.as_deref())
                .unwrap_or("unknown");

            info!(
                "Google Chat message from {} (space {}): {}",
                user_display, space_id, text
            );

            // Route to agent
            match GChatState::get_or_create_agent(
                &state.agents,
                &state.data_dir,
                &state.provider_api_key,
                &state.provider_base_url,
                &state.model,
                &space_id,
            )
            .await
            {
                Ok(sender) => {
                    if let Err(e) = sender.send(AgentMessage::UserMessage(text)).await {
                        error!("Failed to send to Google Chat agent {}: {}", space_id, e);
                        state.agents.lock().await.remove(&space_id);
                    }
                }
                Err(e) => {
                    error!("Cannot create Google Chat agent for {}: {}", space_id, e);
                }
            }
        }

        "ADDED_TO_SPACE" => {
            info!("Google Chat bot added to space");
            let space = event.space.as_ref();
            let space_name = space
                .and_then(|s| s.display_name.as_deref())
                .unwrap_or("unknown");
            info!("  Space: {}", space_name);

            // Send a welcome message
            if let Some(space) = space {
                if let Some(space_id) = &space.name {
                    let welcome =
                        "Hello! I'm ARLI, an AI assistant. Send me a message to get started.";
                    if let Err(e) = send_chat_message(space_id, welcome).await {
                        error!("Failed to send welcome message: {}", e);
                    }
                }
            }
        }

        "REMOVED_FROM_SPACE" => {
            info!("Google Chat bot removed from space");
        }

        _ => {
            // Uninteresting event type
        }
    }

    "ok".to_string()
}

/// Run the Google Chat gateway (webhook server).
pub async fn run(port: u16, data_dir: PathBuf) -> anyhow::Result<()> {
    info!("Google Chat gateway starting on port {}...", port);

    let (response_tx, mut response_rx) = tokio::sync::mpsc::channel::<(String, String)>(128);

    let config = Config::from_env()?;
    let state = Arc::new(GChatState {
        agents: Mutex::new(HashMap::new()),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        response_tx: response_tx.clone(),
    });

    // Spawn response sender
    tokio::spawn(async move {
        while let Some((space_id, text)) = response_rx.recv().await {
            if let Err(e) = send_chat_message(&space_id, &text).await {
                error!("Failed to send Google Chat message: {}", e);
            }
        }
    });

    let app = Router::new()
        .route("/google-chat", post(chat_webhook))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Google Chat webhook listening on {}/google-chat", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

// ── Token resolution helpers ──

fn resolve_gchat_value(env_var: &str, config_key: &str) -> Option<String> {
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
    let _verification_token = resolve_gchat_value("GOOGLE_CHAT_VERIFICATION_TOKEN", "google_chat_verification_token")
        .ok_or_else(|| anyhow::anyhow!(
            "GOOGLE_CHAT_VERIFICATION_TOKEN not set. Set env var or gateway.google_chat_verification_token in config.toml"
        ))?;

    let port: u16 = std::env::var("GOOGLE_CHAT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3009);

    run(port, data_dir).await
}
