//! IRC gateway — TCP client that connects to an IRC server,
//! joins channels, listens for PRIVMSG, and replies.
//!
//! Environment variables:
//!   IRC_SERVER   — IRC server hostname (default: irc.libera.chat)
//!   IRC_PORT     — IRC server port (default: 6667)
//!   IRC_NICK     — Bot nickname
//!   IRC_CHANNELS — Comma-separated list of channels to join
//!
//! Reference: https://modern.ircdocs.horse/

use arli_core::{
    Agent, AgentConfig, AgentMessage, Config,
    OpenAIProvider, SessionStore, ToolRegistry,
    memory::MemoryStore,
};
use arli_core::tools::builtin::register_builtin_tools;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{info, error};

/// Shared state for IRC gateway.
struct IrcState {
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    write: Mutex<tokio::io::WriteHalf<tokio::net::TcpStream>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
    nick: String,
}

impl IrcState {
    async fn get_or_create_agent(
        agents: &Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
        data_dir: &Path,
        provider_api_key: &str,
        provider_base_url: &Option<String>,
        model: &str,
        channel: &str,
        nick: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = agents.lock().await;

        let safe_id = channel.replace(['#', '&', '/', '\\', ' ', '@'], "_");

        if let Some(sender) = agents.get(channel) {
            return Ok(sender.clone());
        }

        info!("Creating IRC agent for channel {}", channel);

        let db_path = data_dir.join(format!("irc-{}.db", safe_id));
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
            name: format!("irc-{}", safe_id),
            session_id: None,
            system_prompt: Some(format!(
                "You are ARLI, an AI agent in the IRC channel {}. \
                 Respond in the user's language. Be concise — IRC messages \
                 should be short, ideally one or two lines. \
                 Use plain text only, no formatting. \
                 Your current nick is {}.",
                channel, nick
            )),
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
        let channel_owned = channel.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(_response) => {
                        // Response handled via response channel
                    }
                    Err(e) => {
                        error!("IRC agent {} error: {}", channel_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(channel.to_string(), sender.clone());
        Ok(sender)
    }
}

/// Send a raw IRC message.
async fn irc_send(writer: &Mutex<tokio::io::WriteHalf<TcpStream>>, msg: &str) -> anyhow::Result<()> {
    let mut w = writer.lock().await;
    w.write_all(format!("{}\r\n", msg).as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

/// Send a PRIVMSG to a channel or user.
async fn irc_privmsg(
    writer: &Mutex<tokio::io::WriteHalf<TcpStream>>,
    target: &str,
    text: &str,
) -> anyhow::Result<()> {
    // IRC messages should be short; limit to ~400 chars per line
    let truncated = if text.len() > 400 {
        format!("{}...", &text[..397])
    } else {
        text.to_string()
    };

    // Split on newlines and send each line separately
    for line in truncated.lines() {
        let line = line.trim();
        if !line.is_empty() {
            irc_send(writer, &format!("PRIVMSG {} :{}", target, line)).await?;
        }
    }

    Ok(())
}

/// Parse IRC line: ":nick!user@host PRIVMSG #channel :message"
fn parse_privmsg(line: &str) -> Option<(String, String, String)> {
    // Remove leading ':'
    let line = line.strip_prefix(':')?;

    // Split at first space to get prefix
    let (prefix, rest) = line.split_once(' ')?;

    // Extract nick from prefix (nick!user@host)
    let nick = prefix.split('!').next()?.to_string();

    // Parse command and args
    let (command, rest) = rest.split_once(' ')?;

    if command != "PRIVMSG" {
        return None;
    }

    // Target (channel or nick)
    let (target, rest) = rest.split_once(" :")?;

    let channel = target.to_string();
    let message = rest.to_string();

    Some((nick, channel, message))
}

/// Run the IRC gateway.
pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let server = std::env::var("IRC_SERVER").unwrap_or_else(|_| "irc.libera.chat".to_string());
    let port: u16 = std::env::var("IRC_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(6667);

    let nick = std::env::var("IRC_NICK")
        .map_err(|_| anyhow::anyhow!("IRC_NICK not set"))?;

    let channels: Vec<String> = std::env::var("IRC_CHANNELS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if channels.is_empty() {
        anyhow::bail!("IRC_CHANNELS not set or empty");
    }

    info!("IRC gateway connecting to {}:{}...", server, port);

    let config = Config::from_env()?;
    let stream = TcpStream::connect(format!("{}:{}", server, port)).await?;
    let (read_half, write_half) = tokio::io::split(stream);

    let state = Arc::new(IrcState {
        agents: Mutex::new(HashMap::new()),
        write: Mutex::new(write_half),
        data_dir: data_dir.clone(),
        provider_api_key: config.provider.api_key.clone(),
        provider_base_url: config.provider.base_url.clone(),
        model: config.model.clone(),
        nick: nick.clone(),
    });

    // Register with the IRC server
    irc_send(&state.write, &format!("NICK {}", nick)).await?;
    irc_send(
        &state.write,
        &format!("USER {} 0 * :ARLI Gateway Bot", nick),
    )
    .await?;

    // Wait for MOTD end, then join channels
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                error!("IRC connection closed during registration");
                anyhow::bail!("Connection closed");
            }
            Ok(_) => {
                let trimmed = line.trim();
                info!("IRC: {}", trimmed);

                // Handle PING
                if trimmed.starts_with("PING ") {
                    let pong = trimmed.replacen("PING", "PONG", 1);
                    irc_send(&state.write, &pong).await?;
                }

                // End of MOTD — join channels
                if trimmed.contains(" 376 ") || trimmed.contains(" 422 ") {
                    for channel in &channels {
                        irc_send(&state.write, &format!("JOIN {}", channel)).await?;
                        info!("Joined IRC channel {}", channel);
                    }
                    break;
                }
            }
            Err(e) => {
                anyhow::bail!("IRC read error: {}", e);
            }
        }
    }

    // Main message loop
    let _state_clone = state.clone();
    let nick_clone = nick.clone();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                error!("IRC connection closed");
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();

                // Handle PING
                if trimmed.starts_with("PING ") {
                    let pong = trimmed.replacen("PING", "PONG", 1);
                    irc_send(&state.write, &pong).await?;
                    continue;
                }

                // Parse PRIVMSG
                if let Some((sender_nick, channel, message)) = parse_privmsg(trimmed) {
                    // Don't respond to our own messages
                    if sender_nick == nick_clone {
                        continue;
                    }

                    info!("IRC <{}> {}: {}", channel, sender_nick, message);

                    match IrcState::get_or_create_agent(
                        &state.agents,
                        &state.data_dir,
                        &state.provider_api_key,
                        &state.provider_base_url,
                        &state.model,
                        &channel,
                        &nick_clone,
                    )
                    .await
                    {
                        Ok(sender) => {
                            let chat_prefix = format!("[{}] {}", sender_nick, message);
                            if let Err(e) = sender
                                .send(AgentMessage::UserMessage(chat_prefix))
                                .await
                            {
                                error!(
                                    "Failed to send to IRC agent {}: {}",
                                    channel, e
                                );
                                state.agents.lock().await.remove(&channel);
                            }
                        }
                        Err(e) => {
                            error!(
                                "Cannot create IRC agent for {}: {}",
                                channel, e
                            );
                        }
                    }
                }
            }
            Err(e) => {
                error!("IRC read error: {}", e);
                break;
            }
        }
    }

    Ok(())
}
