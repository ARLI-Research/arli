//! BlueBubbles (iMessage) gateway — polls a BlueBubbles server for new
//! iMessage messages and replies via the BlueBubbles REST API.
//!
//! Environment variables:
//!   BLUEBUBBLES_SERVER   — BlueBubbles server URL (default: http://localhost:1234)
//!   BLUEBUBBLES_PASSWORD — BlueBubbles server password
//!
//! Note: BlueBubbles requires a macOS host with iMessage.
//! Reference: https://bluebubbles.app/

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    memory::MemoryStore, Agent, AgentConfig, AgentMessage, Config, OpenAIProvider, SessionStore,
    ToolRegistry,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

// ── BlueBubbles API types ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BbApiResponse<T> {
    data: Option<T>,
    status: Option<i32>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BbChat {
    guid: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "lastMessage")]
    last_message: Option<BbMessage>,
    participants: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Clone)]
struct BbMessage {
    guid: Option<String>,
    text: Option<String>,
    #[serde(rename = "dateCreated")]
    date_created: Option<i64>,
    #[serde(rename = "isFromMe")]
    is_from_me: Option<bool>,
    #[serde(rename = "chatGuid")]
    chat_guid: Option<String>,
    #[serde(rename = "handleId")]
    handle_id: Option<i64>,
    sender: Option<String>,
}

#[derive(Debug, Serialize)]
struct BbSendRequest {
    #[serde(rename = "chatGuid")]
    chat_guid: String,
    message: String,
    method: String,
}

struct BlueBubblesGateway {
    server: String,
    password: String,
    agents: Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>,
    seen_messages: Mutex<Vec<String>>,
    data_dir: PathBuf,
    provider_api_key: String,
    provider_base_url: Option<String>,
    model: String,
}

impl BlueBubblesGateway {
    fn new(server: String, password: String, data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = Config::from_env()?;
        Ok(Self {
            server: server.trim_end_matches('/').to_string(),
            password,
            agents: Mutex::new(HashMap::new()),
            seen_messages: Mutex::new(Vec::new()),
            data_dir,
            provider_api_key: config.provider.api_key,
            provider_base_url: config.provider.base_url,
            model: config.model,
        })
    }

    /// Poll /api/v1/chat for all chats with their last messages.
    async fn poll_chats(&self) -> anyhow::Result<Vec<BbChat>> {
        let url = format!("{}/api/v1/chat", self.server);
        let client = reqwest::Client::new();

        let resp = client
            .get(&url)
            .header("password", &self.password)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            warn!(
                "BlueBubbles poll returned {}: {}",
                status,
                resp.text().await.unwrap_or_default()
            );
            return Ok(vec![]);
        }

        let api_resp: BbApiResponse<Vec<BbChat>> = resp.json().await?;
        Ok(api_resp.data.unwrap_or_default())
    }

    /// Poll /api/v1/chat/{guid}/message for recent messages in a chat.
    async fn poll_chat_messages(&self, chat_guid: &str) -> anyhow::Result<Vec<BbMessage>> {
        let url = format!("{}/api/v1/chat/{}/message", self.server, chat_guid);
        let client = reqwest::Client::new();

        let resp = client
            .get(&url)
            .header("password", &self.password)
            .query(&[("limit", "10")])
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            return Ok(vec![]);
        }

        let api_resp: BbApiResponse<Vec<BbMessage>> = resp.json().await?;
        Ok(api_resp.data.unwrap_or_default())
    }

    /// Send a message via /api/v1/message/text.
    async fn send_message(&self, chat_guid: &str, text: &str) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/message/text", self.server);

        let truncated = if text.len() > 4000 {
            format!("{}... _(truncated)_", &text[..4000])
        } else {
            text.to_string()
        };

        let req = BbSendRequest {
            chat_guid: chat_guid.to_string(),
            message: truncated,
            method: "apple-script".to_string(),
        };

        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .header("password", &self.password)
            .json(&req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            warn!(
                "BlueBubbles send error ({}): {}",
                status,
                resp.text().await.unwrap_or_default()
            );
        }

        Ok(())
    }

    async fn get_or_create_agent(
        &self,
        chat_guid: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Sender<AgentMessage>> {
        let mut agents = self.agents.lock().await;

        if let Some(sender) = agents.get(chat_guid) {
            return Ok(sender.clone());
        }

        let safe_id = chat_guid.replace(['/', ' ', ':', '@', '\\'], "_");
        info!("Creating BlueBubbles agent for chat {}", chat_guid);

        let db_path = self.data_dir.join(format!("imessage-{}.db", safe_id));
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

        let agent_config = AgentConfig {
            name: format!("imessage-{}", safe_id),
            session_id: None,
            system_prompt: Some(
                "You are ARLI, an AI agent communicating via iMessage. \
                 Respond in the user's language. Be concise — iMessage messages are best short."
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
        let server = self.server.clone();
        let password = self.password.clone();
        let chat_owned = chat_guid.to_string();

        tokio::spawn(async move {
            loop {
                match agent.run(None).await {
                    Ok(response) => {
                        let truncated = if response.len() > 4000 {
                            format!("{}... _(truncated)_", &response[..4000])
                        } else {
                            response
                        };
                        let url = format!("{}/api/v1/message/text", server);
                        let req = BbSendRequest {
                            chat_guid: chat_owned.clone(),
                            message: truncated,
                            method: "apple-script".to_string(),
                        };
                        if let Err(e) = reqwest::Client::new()
                            .post(&url)
                            .header("password", &password)
                            .json(&req)
                            .send()
                            .await
                        {
                            error!("Failed to send BlueBubbles response: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("BlueBubbles agent error for chat {}: {}", chat_owned, e);
                        break;
                    }
                }
            }
        });

        agents.insert(chat_guid.to_string(), sender.clone());
        Ok(sender)
    }

    async fn process_chat(&self, chat: &BbChat) {
        let chat_guid = match &chat.guid {
            Some(g) => g,
            None => return,
        };

        let last_msg = match &chat.last_message {
            Some(m) => m,
            None => return,
        };

        // Skip messages from ourselves
        if last_msg.is_from_me == Some(true) {
            return;
        }

        let msg_guid = match &last_msg.guid {
            Some(g) => g,
            None => return,
        };

        // Skip already-seen messages
        {
            let seen = self.seen_messages.lock().await;
            if seen.contains(msg_guid) {
                return;
            }
        }

        let text = match &last_msg.text {
            Some(t) => t.trim().to_string(),
            None => return,
        };

        if text.is_empty() {
            return;
        }

        let sender = last_msg.sender.as_deref().unwrap_or("unknown");
        info!(
            "BlueBubbles message from {} (chat {}): {}",
            sender, chat_guid, text
        );

        // Mark as seen
        {
            let mut seen = self.seen_messages.lock().await;
            seen.push(msg_guid.clone());
            // Keep only last 1000
            let len = seen.len();
            if len > 1000 {
                seen.drain(0..len - 1000);
            }
        }

        match self.get_or_create_agent(chat_guid).await {
            Ok(agent_sender) => {
                if let Err(e) = agent_sender.send(AgentMessage::UserMessage(text)).await {
                    error!("Failed to send to BlueBubbles agent {}: {}", chat_guid, e);
                    self.agents.lock().await.remove(chat_guid);
                }
            }
            Err(e) => {
                error!("Cannot create BlueBubbles agent for {}: {}", chat_guid, e);
            }
        }
    }

    async fn run_forever(self: Arc<Self>) {
        info!("BlueBubbles gateway starting, server: {}", self.server);

        loop {
            match self.poll_chats().await {
                Ok(chats) => {
                    for chat in chats {
                        self.process_chat(&chat).await;
                    }
                }
                Err(e) => {
                    error!("BlueBubbles poll error: {}", e);
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }
}

pub async fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    let server =
        std::env::var("BLUEBUBBLES_SERVER").unwrap_or_else(|_| "http://localhost:1234".to_string());

    let password = std::env::var("BLUEBUBBLES_PASSWORD").ok().ok_or_else(|| {
        anyhow::anyhow!("BLUEBUBBLES_PASSWORD not set. Set BLUEBUBBLES_PASSWORD env var.")
    })?;

    let gateway = Arc::new(BlueBubblesGateway::new(server, password, data_dir)?);
    gateway.run_forever().await;
    Ok(())
}
