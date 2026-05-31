//! Webhook subscriptions — HTTP endpoints that trigger agent runs.
//!
//! External services (CI/CD, monitoring, GitHub, etc.) can POST to
//! ARLI webhook endpoints to trigger automated agent workflows.
//!
//! Each subscription has a name and optional prompt template.
//! The webhook payload is available as JSON in the agent message.

use crate::agent::AgentMessage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// A webhook subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscription {
    pub name: String,
    /// Prompt template — `{{payload}}` is replaced with the JSON body.
    pub prompt_template: String,
    /// Target channel for agent output (chat_id, etc.). None = no delivery.
    #[serde(default)]
    pub target_channel: Option<String>,
}

/// Shared state for the webhook server.
pub struct WebhookState {
    subscriptions: Mutex<HashMap<String, WebhookSubscription>>,
    agent_tx: Option<tokio::sync::mpsc::Sender<(String, String)>>,
}

impl WebhookState {
    pub fn new() -> Self {
        Self {
            subscriptions: Mutex::new(HashMap::new()),
            agent_tx: None,
        }
    }

    pub fn with_agent_channel(tx: tokio::sync::mpsc::Sender<(String, String)>) -> Self {
        Self {
            subscriptions: Mutex::new(HashMap::new()),
            agent_tx: Some(tx),
        }
    }

    /// Add a subscription.
    pub async fn subscribe(&self, sub: WebhookSubscription) {
        self.subscriptions.lock().await.insert(sub.name.clone(), sub);
    }

    /// Remove a subscription.
    pub async fn unsubscribe(&self, name: &str) -> bool {
        self.subscriptions.lock().await.remove(name).is_some()
    }

    /// List all subscriptions.
    pub async fn list(&self) -> Vec<WebhookSubscription> {
        self.subscriptions.lock().await.values().cloned().collect()
    }

    /// Handle an incoming webhook POST.
    pub async fn handle(&self, name: &str, body: String) -> anyhow::Result<String> {
        let subs = self.subscriptions.lock().await;
        let sub = subs.get(name)
            .ok_or_else(|| anyhow::anyhow!("Webhook '{}' not found", name))?;

        let message = sub.prompt_template.replace("{{payload}}", &body);

        if let Some(tx) = &self.agent_tx {
            tx.send((name.to_string(), message)).await
                .map_err(|e| anyhow::anyhow!("Agent channel closed: {}", e))?;
            Ok("Queued for agent processing".to_string())
        } else {
            // No agent channel — just return what would have been sent
            info!("Webhook '{}' received (no agent channel configured)", name);
            Ok(format!("Webhook received. Payload size: {} bytes", body.len()))
        }
    }
}

/// Run a webhook HTTP server.
pub async fn serve(state: Arc<WebhookState>, port: u16) -> anyhow::Result<()> {
    use axum::{
        extract::{Path, State},
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };

    async fn list_handler(State(state): State<Arc<WebhookState>>) -> impl IntoResponse {
        let subs = state.list().await;
        Json(subs)
    }

    async fn webhook_handler(
        State(state): State<Arc<WebhookState>>,
        Path(name): Path<String>,
        body: String,
    ) -> impl IntoResponse {
        match state.handle(&name, body).await {
            Ok(msg) => (axum::http::StatusCode::OK, msg),
            Err(e) => (axum::http::StatusCode::NOT_FOUND, e.to_string()),
        }
    }

    let app = Router::new()
        .route("/webhooks", get(list_handler))
        .route("/webhooks/{name}", post(webhook_handler))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("Webhook server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subscribe_and_handle() {
        let state = WebhookState::new();

        state.subscribe(WebhookSubscription {
            name: "test-hook".to_string(),
            prompt_template: "Process this: {{payload}}".to_string(),
            target_channel: None,
        }).await;

        let result = state.handle("test-hook", r#"{"key":"value"}"#.to_string()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_missing_webhook() {
        let state = WebhookState::new();
        let result = state.handle("nonexistent", "{}".to_string()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let state = WebhookState::new();
        state.subscribe(WebhookSubscription {
            name: "temp".to_string(),
            prompt_template: "test".to_string(),
            target_channel: None,
        }).await;

        assert!(state.unsubscribe("temp").await);
        assert!(!state.unsubscribe("temp").await);
    }
}
