//! Microsoft Teams gateway — Bot Framework webhook receiver + message responder.
//!
//! Uses the Microsoft Bot Framework REST API. Requires an Azure Bot registration
//! with a Microsoft Teams channel.
//!
//! Environment variables:
//!   MS_TEAMS_APP_ID       — Azure Bot app ID
//!   MS_TEAMS_APP_PASSWORD — Azure Bot app password / client secret
//!   MS_TEAMS_PORT         — Webhook listen port (default: 3007)
//!
//! Reference: https://learn.microsoft.com/en-us/microsoftteams/platform/bots/

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

// ── Bot Framework types ──

/// Inbound activity from Teams (Bot Framework schema).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Activity {
    #[serde(rename = "type")]
    activity_type: Option<String>,
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "serviceUrl")]
    service_url: Option<String>,
    #[serde(rename = "channelId")]
    channel_id: Option<String>,
    from: Option<ChannelAccount>,
    conversation: Option<ConversationAccount>,
    text: Option<String>,
    #[serde(rename = "textFormat")]
    #[allow(dead_code)]
    text_format: Option<String>,
    #[serde(rename = "replyToId")]
    reply_to_id: Option<String>,
    #[serde(rename = "membersAdded")]
    members_added: Option<Vec<ChannelAccount>>,
    #[serde(rename = "membersRemoved")]
    members_removed: Option<Vec<ChannelAccount>>,
}

#[derive(Debug, Deserialize, Clone)]
struct ChannelAccount {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ConversationAccount {
    id: Option<String>,
    name: Option<String>,
    #[serde(rename = "conversationType")]
    #[allow(dead_code)]
    conversation_type: Option<String>,
}

/// Outbound activity for sending replies.
#[derive(Debug, Serialize)]
struct ReplyActivity {
    #[serde(rename = "type")]
    activity_type: String,
    from: ChannelAccountRef,
    text: String,
    #[serde(rename = "textFormat")]
    text_format: String,
}

#[derive(Debug, Serialize)]
struct ChannelAccountRef {
    id: String,
    name: String,
}

/// OAuth2 token response from Microsoft login.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[allow(dead_code)]
    token_type: String,
    #[allow(dead_code)]
    expires_in: i64,
}

/// Shared state for Teams webhook handler.
struct TeamsState {
    /// Per-conversation agent senders, keyed by conversation ID.
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    /// Response channel: (conversation_id, service_url, text).
    response_tx: tokio::sync::mpsc::Sender<(String, String, String)>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    /// Azure Bot credentials.
    app_id: String,
    app_password: String,
}

impl TeamsState {
    #[allow(clippy::too_many_arguments)]
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        conversation_id: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        // Sanitize conversation ID for filesystem use.
        let safe_id = conversation_id.replace([':', '/', '\\', ' ', '@'], "_");

        if let Some(sender) = agents.get(conversation_id) {
            return Ok(sender.clone());
        }

        info!("Creating Teams agent for conversation {}", conversation_id);

        let db_path = data_dir.join(format!("teams-{}.db", safe_id));
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
            name: format!("teams-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via Microsoft Teams. \
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
        let conversation_owned = conversation_id.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(_response) => {
                        // Response is handled by the response channel
                    }
                    Err(e) => {
                        error!("Teams agent {} error: {}", conversation_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(conversation_id.to_string(), sender.clone());
        Ok(sender)
    }
}

// ── Bot Framework token acquisition ──

/// Obtain an OAuth2 bearer token from the Bot Framework's Azure AD endpoint.
async fn get_bot_token(app_id: &str, app_password: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let params = [
        ("grant_type", "client_credentials"),
        ("client_id", app_id),
        ("client_secret", app_password),
        ("scope", "https://api.botframework.com/.default"),
    ];

    let resp: TokenResponse = client
        .post("https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token")
        .form(&params)
        .send()
        .await?
        .json()
        .await?;

    Ok(resp.access_token)
}

/// Send a reply to a Teams conversation via Bot Framework REST API.
async fn send_teams_reply(
    app_id: &str,
    app_password: &str,
    service_url: &str,
    conversation_id: &str,
    text: &str,
) -> anyhow::Result<()> {
    let token = get_bot_token(app_id, app_password).await?;

    let truncated = if text.len() > 4000 {
        format!("{}... _(truncated)_", &text[..3990])
    } else {
        text.to_string()
    };

    let url = format!(
        "{}/v3/conversations/{}/activities",
        service_url.trim_end_matches('/'),
        conversation_id
    );

    let reply = ReplyActivity {
        activity_type: "message".to_string(),
        from: ChannelAccountRef {
            id: app_id.to_string(),
            name: "ARLI".to_string(),
        },
        text: truncated,
        text_format: "markdown".to_string(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&reply)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        warn!("Teams send error ({}): {}", status, body);
    }

    Ok(())
}

// ── Webhook handlers ──

/// Handle incoming Teams activity (POST /api/messages).
async fn teams_webhook(
    State(state): State<Arc<TeamsState>>,
    Json(activity): Json<Activity>,
) -> impl IntoResponse {
    let activity_type = activity.activity_type.as_deref().unwrap_or("unknown");

    match activity_type {
        "message" => {
            let text = match &activity.text {
                Some(t) => t.trim().to_string(),
                None => return "ok",
            };

            if text.is_empty() {
                return "ok";
            }

            let conversation = match &activity.conversation {
                Some(c) => c,
                None => return "ok",
            };

            let conversation_id = match &conversation.id {
                Some(id) => id.clone(),
                None => return "ok",
            };

            let user = activity
                .from
                .as_ref()
                .and_then(|f| f.name.as_deref())
                .unwrap_or("unknown");

            info!(
                "Teams message from {} (conv {}): {}",
                user, conversation_id, text
            );

            // Route to agent.
            match TeamsState::get_or_create_agent(
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
                        error!("Failed to send to Teams agent {}: {}", conversation_id, e);
                        state.agents.lock().await.remove(&conversation_id);
                    }
                }
                Err(e) => {
                    error!("Cannot create Teams agent for {}: {}", conversation_id, e);
                }
            }
        }

        "conversationUpdate" => {
            // Handle bot being added to a conversation.
            if let Some(members) = &activity.members_added {
                for member in members {
                    if member.id.as_deref() == Some(&state.app_id) {
                        info!("Teams bot added to conversation");
                    }
                }
            }
        }

        _ => {
            // Uninteresting activity type — acknowledged silently.
        }
    }

    // Always return 200 OK so Teams doesn't retry.
    "ok"
}

// ── Public entry point ──

/// Run the Microsoft Teams gateway (webhook server).
pub async fn run(
    app_id: String,
    app_password: String,
    webhook_port: u16,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    info!("Teams gateway starting on port {}...", webhook_port);

    let (response_tx, mut response_rx) =
        tokio::sync::mpsc::channel::<(String, String, String)>(128);

    let config = Config::from_env()?;
    let state = Arc::new(TeamsState {
        agents: Mutex::new(HashMap::new()),
        response_tx: response_tx.clone(),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        app_id: app_id.clone(),
        app_password: app_password.clone(),
    });

    // Spawn response sender — polls the response channel and sends replies
    // via the Bot Framework REST API.
    let aid = app_id.clone();
    let apw = app_password.clone();
    tokio::spawn(async move {
        while let Some((conversation_id, service_url, text)) = response_rx.recv().await {
            if let Err(e) =
                send_teams_reply(&aid, &apw, &service_url, &conversation_id, &text).await
            {
                error!("Failed to send Teams reply: {}", e);
            }
        }
    });

    // Build axum router — only POST, no webhook verification needed for
    // Bot Framework (trust is established via Azure AD).
    let app = Router::new()
        .route("/api/messages", post(teams_webhook))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", webhook_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Teams webhook listening on {}/api/messages", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

// ── Token resolution helpers ──

/// Resolve a Teams-related value from env or config.toml.
fn resolve_teams_value(env_var: &str, config_key: &str) -> Option<String> {
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

/// Convenience wrapper that reads MS_TEAMS_APP_ID/MS_TEAMS_APP_PASSWORD
/// and starts the gateway.
pub async fn run_from_env(data_dir: PathBuf) -> anyhow::Result<()> {
    let app_id = resolve_teams_value("MS_TEAMS_APP_ID", "teams_app_id").ok_or_else(|| {
        anyhow::anyhow!(
            "MS_TEAMS_APP_ID not set. Set MS_TEAMS_APP_ID env var or \
             gateway.teams_app_id in config.toml"
        )
    })?;

    let app_password = resolve_teams_value("MS_TEAMS_APP_PASSWORD", "teams_app_password")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "MS_TEAMS_APP_PASSWORD not set. Set MS_TEAMS_APP_PASSWORD env var or \
             gateway.teams_app_password in config.toml"
            )
        })?;

    let port: u16 = std::env::var("MS_TEAMS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3007);

    run(app_id, app_password, port, data_dir).await
}
