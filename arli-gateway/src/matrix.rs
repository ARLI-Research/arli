//! Matrix gateway — bot that routes room messages to ARLI agents.
//!
//! Connects to a Matrix homeserver (default: matrix.org), logs in as a bot
//! user, and listens for room messages. Each room gets its own Agent with
//! session persistence, and agent responses are forwarded back to the room.
//!
//! Configuration (env vars):
//!   MATRIX_HOMESERVER   — Homeserver URL (default: https://matrix.org)
//!   MATRIX_USER         — Bot username (e.g. @arli-bot:matrix.org)
//!   MATRIX_PASSWORD     — Bot password
//!   MATRIX_ACCESS_TOKEN — Pre-obtained access token (alternative to password)

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use matrix_sdk::{
    config::SyncSettings,
    event_handler::Ctx,
    ruma::events::room::message::{
        MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent,
    },
    Client, Room, RoomState,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};

/// Per-room agent state.
struct AgentRoomState {
    /// Sender to push user messages into the agent's mailbox.
    user_tx: tokio::sync::mpsc::Sender<AgentMessage>,
}

/// Shared bot state.
struct MatrixBot {
    rooms: Mutex<HashMap<String, AgentRoomState>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl MatrixBot {
    fn new(data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            rooms: Mutex::new(HashMap::new()),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

    /// Get or create an agent for a room.
    /// Returns a sender to push messages into the agent's mailbox.
    async fn get_or_create(
        &self,
        room_id: &str,
        room: Room,
        room_name: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut rooms = self.rooms.lock().await;

        if let Some(state) = rooms.get(room_id) {
            return Ok(state.user_tx.clone());
        }

        info!("Creating Matrix agent for room {}", room_id);

        // Sanitize room_id for use in filenames
        let safe_id = room_id
            .replace(':', "_")
            .replace('.', "_")
            .replace('!', "_");
        let db_path = self.data_dir.join(format!("matrix-{}.db", safe_id));
        let store = SessionStore::open(db_path.clone())?;
        let memory_path = self.data_dir.join("memory.db");
        let memory_store = Arc::new(MemoryStore::open(memory_path)?);

        let provider = Box::new(OpenAIProvider::new(
            self.provider_api_key.clone(),
            self.model.clone(),
            self.provider_base_url.clone(),
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

        let system_prompt = format!(
            "You are ARLI, an AI agent communicating via Matrix in room '{}'. \
             Respond in the user's language. Be concise.",
            room_name
        );

        let agent_config = AgentConfig {
            brokering: None,
            tenant_id: None,
            provider_name: None,
            name: format!("matrix-{}", safe_id),
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

        let user_tx = agent.sender();
        let room_id_owned = room_id.to_string();

        // Spawn per-room agent loop
        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        if !response.is_empty() {
                            let text = if response.len() > 4000 {
                                let mut truncated = response[..3990].to_string();
                                truncated.push_str("... _(truncated)_");
                                truncated
                            } else {
                                response
                            };

                            let content = RoomMessageEventContent::text_plain(text);
                            if let Err(e) = room.send(content).await {
                                error!(
                                    "Failed to send Matrix response to {}: {}",
                                    room_id_owned, e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!("Matrix agent {} error: {}", room_id_owned, e);
                        let content =
                            RoomMessageEventContent::text_plain(format!("⚠️ Agent error: {}", e));
                        let _ = room.send(content).await;
                        break;
                    }
                }
            }
        });

        let state = AgentRoomState {
            user_tx: user_tx.clone(),
        };
        rooms.insert(room_id.to_string(), state);
        Ok(user_tx)
    }

    async fn remove(&self, room_id: &str) {
        self.rooms.lock().await.remove(room_id);
        info!("Removed Matrix agent {}", room_id);
    }

    /// Process an incoming room message.
    async fn handle_message(&self, room: Room, room_id: &str, sender: &str, body: &str) {
        // Ignore our own messages
        if let Some(own_user) = room.client().user_id() {
            if sender == own_user.as_str() {
                return;
            }
        }

        let content = body.trim().to_string();
        if content.is_empty() {
            return;
        }

        // Try to get room display name, fall back to room ID
        let room_name = room
            .cached_display_name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| room_id.to_string());

        info!("Matrix: {} in {}: {}", sender, room_name, content);

        // Commands
        if content == "!arli start" || content == "!start" {
            let _ = room
                .send(RoomMessageEventContent::text_plain("ARLI agent is ready."))
                .await;
            return;
        }

        if content == "!arli help" || content == "!help" {
            let help = "**ARLI Agent**\n\
                `!arli start` — Initialize\n\
                `!arli reset` — Reset conversation\n\
                `!arli help` — This help";
            let _ = room.send(RoomMessageEventContent::text_plain(help)).await;
            return;
        }

        if content == "!arli reset" || content == "!reset" {
            self.remove(room_id).await;
            let _ = room
                .send(RoomMessageEventContent::text_plain("Conversation reset."))
                .await;
            return;
        }

        // Route to agent
        match self.get_or_create(room_id, room.clone(), &room_name).await {
            Ok(tx) => {
                // Send typing notification
                let _ = room.typing_notice(true).await;
                if let Err(e) = tx.send(AgentMessage::UserMessage(content)).await {
                    error!("Failed to send to agent {}: {}", room_id, e);
                    self.remove(room_id).await;
                }
            }
            Err(e) => {
                error!("Cannot create agent for {}: {}", room_id, e);
                let _ = room
                    .send(RoomMessageEventContent::text_plain("Agent init failed."))
                    .await;
            }
        }
    }
}

/// Read Matrix configuration from environment variables.
fn resolve_matrix_config() -> Option<(String, String, String)> {
    let homeserver =
        std::env::var("MATRIX_HOMESERVER").unwrap_or_else(|_| "https://matrix.org".to_string());
    let user = std::env::var("MATRIX_USER").ok()?;
    let password = std::env::var("MATRIX_PASSWORD").ok();
    let access_token = std::env::var("MATRIX_ACCESS_TOKEN").ok();

    // Password or access token must be provided
    let auth = password.or(access_token)?;
    Some((homeserver, user, auth))
}

/// Run the Matrix gateway.
pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let (homeserver, user, auth) = resolve_matrix_config().ok_or_else(|| {
        anyhow::anyhow!(
            "Matrix credentials not configured. Set MATRIX_USER and either \
             MATRIX_PASSWORD or MATRIX_ACCESS_TOKEN environment variables."
        )
    })?;

    info!("Matrix gateway starting (homeserver: {})", homeserver);

    let bot = Arc::new(MatrixBot::new(data_dir)?);

    // Build the Matrix client
    let client = Client::builder()
        .homeserver_url(&homeserver)
        .build()
        .await?;

    // Login — uses password auth (MATRIX_PASSWORD).
    // For access tokens (MATRIX_ACCESS_TOKEN), this uses password login
    // as a fallback since matrix-sdk 0.9 session restoration requires
    // types not easily available from the public API.
    info!("Matrix: logging in as {}", user);
    client
        .matrix_auth()
        .login_username(&user, &auth)
        .initial_device_display_name("ARLI Gateway")
        .send()
        .await?;

    info!(
        "Matrix bot logged in as {}",
        client.user_id().map(|u| u.to_string()).unwrap_or_default()
    );

    // Add event handler for room messages
    let bot_clone = Arc::clone(&bot);
    client.add_event_handler(
        move |ev: OriginalSyncRoomMessageEvent, room: Room, Ctx(_client): Ctx<Client>| {
            let bot = Arc::clone(&bot_clone);
            async move {
                // Only process text messages from joined rooms
                if room.state() != RoomState::Joined {
                    return;
                }

                let (body, sender) = match &ev.content.msgtype {
                    MessageType::Text(text_content) => {
                        (text_content.body.clone(), ev.sender.to_string())
                    }
                    MessageType::Notice(notice_content) => {
                        (notice_content.body.clone(), ev.sender.to_string())
                    }
                    _ => return,
                };

                let room_id = room.room_id().to_string();
                bot.handle_message(room, &room_id, &sender, &body).await;
            }
        },
    );

    // Sync loop — long-poll for new events
    info!("Matrix gateway listening for messages...");
    let _ = client.sync(SyncSettings::default()).await;

    Ok(())
}
