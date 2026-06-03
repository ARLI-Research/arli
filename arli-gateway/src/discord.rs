//! Discord gateway — bot that routes messages to ARLI agents.
//!
//! Each Discord channel gets its own Agent with session persistence.
//! Agent responses are forwarded back to the Discord channel.

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::{GatewayIntents, Ready};
use serenity::model::id::ChannelId;
use serenity::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};

/// Per-channel agent state.
struct ChannelState {
    /// Sender to push user messages into the agent's mailbox.
    user_tx: tokio::sync::mpsc::Sender<AgentMessage>,
}

/// Shared bot state.
struct DiscordBot {
    channels: Mutex<HashMap<String, ChannelState>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl DiscordBot {
    fn new(data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            channels: Mutex::new(HashMap::new()),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

    fn channel_key(guild_id: Option<u64>, channel_id: u64) -> String {
        match guild_id {
            Some(gid) => format!("g{}-c{}", gid, channel_id),
            None => format!("dm-{}", channel_id),
        }
    }

    /// Get or create an agent for a channel.
    /// Returns a sender to push messages into the agent's mailbox.
    async fn get_or_create(
        &self,
        key: &str,
        channel_id: u64,
        http: Arc<serenity::http::Http>,
        channel_name: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut channels = self.channels.lock().await;

        if let Some(state) = channels.get(key) {
            return Ok(state.user_tx.clone());
        }

        info!("Creating Discord agent for {}", key);

        let db_path = self.data_dir.join(format!("discord-{}.db", key));
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
            "You are ARLI, an AI agent communicating via Discord in channel '{}'. \
             Respond in the user's language. Be concise.",
            channel_name
        );

        let agent_config = AgentConfig {
            brokering: None,
            tenant_id: None,
            provider_name: None,
            name: format!("discord-{}", key),
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
        let key_owned = key.to_string();

        // Spawn per-channel agent loop
        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        if !response.is_empty() {
                            let text = if response.len() > 2000 {
                                let mut truncated = response[..1990].to_string();
                                truncated.push_str("... _(truncated)_");
                                truncated
                            } else {
                                response
                            };

                            if let Err(e) = ChannelId::from(channel_id).say(&http, &text).await {
                                error!("Failed to send Discord response to {}: {}", key_owned, e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("Discord agent {} error: {}", key_owned, e);
                        let _ = ChannelId::from(channel_id)
                            .say(&http, format!("⚠️ Agent error: {}", e))
                            .await;
                        break;
                    }
                }
            }
        });

        let state = ChannelState {
            user_tx: user_tx.clone(),
        };
        channels.insert(key.to_string(), state);
        Ok(user_tx)
    }

    async fn remove(&self, key: &str) {
        self.channels.lock().await.remove(key);
        info!("Removed Discord agent {}", key);
    }
}

/// Serenity EventHandler.
struct Handler {
    bot: Arc<DiscordBot>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let content = msg.content.trim().to_string();
        if content.is_empty() {
            return;
        }

        let channel_id = msg.channel_id.get();
        let guild_id = msg.guild_id.map(|g| g.get());
        let key = DiscordBot::channel_key(guild_id, channel_id);
        let channel_name = format!("#{}", channel_id);

        info!(
            "Discord: {} in {}: {}",
            msg.author.name, channel_name, content
        );

        // Commands
        if content == "!arli start" || content == "!start" {
            let _ = msg.channel_id.say(&ctx.http, "ARLI agent is ready.").await;
            return;
        }

        if content == "!arli help" || content == "!help" {
            let help = "**ARLI Agent**\n\
                `!arli start` — Initialize\n\
                `!arli reset` — Reset conversation\n\
                `!arli help` — This help";
            let _ = msg.channel_id.say(&ctx.http, help).await;
            return;
        }

        if content == "!arli reset" || content == "!reset" {
            self.bot.remove(&key).await;
            let _ = msg.channel_id.say(&ctx.http, "Conversation reset.").await;
            return;
        }

        // Route to agent
        match self
            .bot
            .get_or_create(&key, channel_id, ctx.http.clone(), &channel_name)
            .await
        {
            Ok(tx) => {
                let _ = msg.channel_id.broadcast_typing(&ctx.http).await;
                if let Err(e) = tx.send(AgentMessage::UserMessage(content)).await {
                    error!("Failed to send to agent {}: {}", key, e);
                    self.bot.remove(&key).await;
                }
            }
            Err(e) => {
                error!("Cannot create agent for {}: {}", key, e);
                let _ = msg.channel_id.say(&ctx.http, "Agent init failed.").await;
            }
        }
    }

    async fn ready(&self, _: Context, ready: Ready) {
        info!("Discord bot connected as {}", ready.user.name);
    }
}

/// Run the Discord gateway.
pub async fn run(token: String, data_dir: PathBuf) -> anyhow::Result<()> {
    info!("Discord gateway starting...");

    let bot = Arc::new(DiscordBot::new(data_dir)?);

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = serenity::Client::builder(token, intents)
        .event_handler(Handler { bot })
        .await?;

    client.start().await?;

    Ok(())
}
