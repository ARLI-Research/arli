//! HTTP API server for self-service inference brokering.
//!
//! Provides OpenAI-compatible endpoints:
//! - POST /v1/chat/completions — Bearer auth, rate limiting, usage tracking
//! - GET  /v1/usage — current-month usage summary
//! - GET  /health   — liveness check

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::brokering::{
    BrokeringConfig, BrokeringError, BrokeringRouter, RateLimiter, TenantInfo, TenantManager,
    UsageTracker,
};
use crate::inference::InferenceRoute;

// ── Request / Response types ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Value>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub top_p: Option<f64>,
    /// Preferred provider (used as primary; fallbacks come from config).
    #[serde(default)]
    pub provider: Option<String>,
    /// Remaining fields are silently ignored.
    #[serde(flatten)]
    _extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: UsageInfo,
}

#[derive(Debug, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: MessageContent,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct MessageContent {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub tenant_name: String,
    pub month: String,
    pub total_requests: u64,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub total_cost_cents: u64,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub timestamp: String,
}

// ── Internal LLM-call result ──────────────────────────────────────────────

struct LlmResponse {
    content: String,
    finish_reason: Option<String>,
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

// ── Server ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    router: Arc<BrokeringRouter>,
    #[allow(dead_code)]
    config: BrokeringConfig,
}

/// HTTP API server for inference brokering.
///
/// Wraps a [`BrokeringRouter`] and exposes an OpenAI-compatible
/// chat-completions endpoint with per-tenant auth, rate limiting,
/// upstream routing, and usage recording.
pub struct BrokeringApiServer {
    state: AppState,
}

impl BrokeringApiServer {
    /// Create a new API server.
    ///
    /// The server owns a [`BrokeringRouter`] built from the supplied
    /// components.  A `Clone` of `tenant_manager` (which is `Clone`-able)
    /// is moved into the router; the caller may keep their own copy for
    /// admin operations if desired.
    pub fn new(
        config: BrokeringConfig,
        tenant_manager: TenantManager,
        rate_limiter: Arc<RateLimiter>,
        usage_tracker: Arc<UsageTracker>,
    ) -> Self {
        let router = Arc::new(BrokeringRouter::new(
            tenant_manager,
            rate_limiter,
            usage_tracker,
        ));
        Self {
            state: AppState { router, config },
        }
    }

    /// Start the axum HTTP server on the given port.
    /// This call blocks until the server shuts down.
    pub async fn serve(self, port: u16) {
        let app = Router::new()
            .route("/v1/chat/completions", post(chat_completions))
            .route("/v1/usage", get(get_usage))
            .route("/health", get(health))
            .with_state(self.state);

        let addr = format!("0.0.0.0:{}", port);
        info!("Brokering API server starting on {}", addr);

        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind port {}: {}", port, e);
                return;
            }
        };

        info!("Brokering API server listening on port {}", port);

        if let Err(e) = axum::serve(listener, app).await {
            error!("Brokering API server exited: {}", e);
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────

async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ChatCompletionRequest>,
) -> (StatusCode, Json<Value>) {
    // 1. Extract Bearer token
    let api_key = match extract_bearer_token(&headers) {
        Some(k) => k,
        None => {
            return json_error(StatusCode::UNAUTHORIZED, "invalid_request_error", "Missing Authorization header. Use: Bearer <api_key>");
        }
    };

    // 2. Look up tenant by hashed API key
    let tenant = match find_tenant_by_key(state.router.get_tenant_manager(), &api_key) {
        Ok(t) => t,
        Err(e) => {
            return json_error(StatusCode::UNAUTHORIZED, "invalid_request_error", &e);
        }
    };

    // 3. Verify tenant is enabled
    if !tenant.enabled {
        return json_error(
            StatusCode::FORBIDDEN,
            "tenant_disabled",
            &format!("Tenant '{}' is disabled", tenant.name),
        );
    }

    // 4. Rate limit + route
    let primary = req.provider.as_deref().unwrap_or("deepseek");
    let estimated_tokens = estimate_tokens(&req.messages);

    let route = match state.router.route_with_fallback(
        tenant.id,
        primary,
        &[], // no additional fallbacks for now
        Some(&req.model),
        estimated_tokens,
    ) {
        Ok(r) => r,
        Err(BrokeringError::TenantDisabled(id)) => {
            return json_error(
                StatusCode::FORBIDDEN,
                "tenant_disabled",
                &format!("Tenant {} is disabled", id),
            );
        }
        Err(BrokeringError::RateLimitExceeded { detail, .. }) => {
            return json_error(StatusCode::TOO_MANY_REQUESTS, "rate_limit_exceeded", &detail);
        }
        Err(e) => {
            error!("Routing failed: {}", e);
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                &e.to_string(),
            );
        }
    };

    // 5. Call upstream LLM
    let llm = match call_upstream(&route, &req).await {
        Ok(r) => r,
        Err(e) => {
            error!("Upstream LLM call failed: {}", e);
            return json_error(StatusCode::BAD_GATEWAY, "upstream_error", &e);
        }
    };

    // 6. Record usage
    let cost_cents = estimate_cost(
        &route.provider.name,
        llm.prompt_tokens as u64,
        llm.completion_tokens as u64,
    );
    if let Err(e) = state.router.record_completion(
        tenant.id,
        &route.provider.name,
        &route.model,
        llm.prompt_tokens as u64,
        llm.completion_tokens as u64,
        cost_cents,
    ) {
        warn!("Failed to record usage: {}", e);
    }

    // 7. Return OpenAI-compatible response
    let response = ChatCompletionResponse {
        id: format!("chatcmpl-{}", Uuid::new_v4()),
        object: "chat.completion".into(),
        created: Utc::now().timestamp() as u64,
        model: route.model.clone(),
        choices: vec![Choice {
            index: 0,
            message: MessageContent {
                role: "assistant".into(),
                content: llm.content,
            },
            finish_reason: llm.finish_reason.unwrap_or_else(|| "stop".into()),
        }],
        usage: UsageInfo {
            prompt_tokens: llm.prompt_tokens,
            completion_tokens: llm.completion_tokens,
            total_tokens: llm.total_tokens,
        },
    };
    (StatusCode::OK, Json(serde_json::to_value(response).unwrap()))
}

async fn get_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    let api_key = match extract_bearer_token(&headers) {
        Some(k) => k,
        None => {
            return json_error(StatusCode::UNAUTHORIZED, "invalid_request_error", "Missing Authorization header");
        }
    };

    let tenant = match find_tenant_by_key(state.router.get_tenant_manager(), &api_key) {
        Ok(t) => t,
        Err(e) => {
            return json_error(StatusCode::UNAUTHORIZED, "invalid_request_error", &e);
        }
    };

    let now = Utc::now();
    let month_start = now
        .date_naive()
        .with_day(1)
        .expect("day 1 is always valid")
        .and_hms_opt(0, 0, 0)
        .expect("midnight is always valid")
        .and_utc();

    match state.router.get_usage_tracker().get_usage(tenant.id, month_start, now) {
        Ok(summary) => {
            let response = UsageResponse {
                tenant_name: tenant.name,
                month: now.format("%Y-%m").to_string(),
                total_requests: summary.total_requests,
                total_tokens_in: summary.total_tokens_in,
                total_tokens_out: summary.total_tokens_out,
                total_cost_cents: summary.total_cost_cents,
            };
            (StatusCode::OK, Json(serde_json::to_value(response).unwrap()))
        }
        Err(e) => {
            error!("Usage query failed: {}", e);
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                &e.to_string(),
            )
        }
    }
}

async fn health() -> (StatusCode, Json<Value>) {
    let response = HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        timestamp: Utc::now().to_rfc3339(),
    };
    (StatusCode::OK, Json(serde_json::to_value(response).unwrap()))
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Extract a `Bearer <token>` from the Authorization header.
fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    let token = auth.strip_prefix("Bearer ")?;
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Look up a tenant by SHA-256 hash of the provided API key.
fn find_tenant_by_key(tm: &TenantManager, api_key: &str) -> Result<TenantInfo, String> {
    let hash = hex::encode(Sha256::digest(api_key.as_bytes()));
    let tenants = tm.list_tenants().map_err(|e| e.to_string())?;
    tenants
        .into_iter()
        .find(|t| t.api_key_hash == hash)
        .ok_or_else(|| "Invalid API key".to_string())
}

/// Rough token estimate from message content lengths.
fn estimate_tokens(messages: &[Value]) -> u32 {
    let char_count: usize = messages
        .iter()
        .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
        .map(|s| s.len())
        .sum();
    // ~4 chars per token + base overhead
    (char_count as f64 / 4.0).ceil() as u32 + 100
}

/// Call the upstream LLM provider and return the parsed response.
async fn call_upstream(
    route: &InferenceRoute,
    req: &ChatCompletionRequest,
) -> Result<LlmResponse, String> {
    let client = reqwest::Client::new();

    let mut body = serde_json::Map::new();
    body.insert("model".into(), json!(req.model));
    body.insert("messages".into(), json!(req.messages));
    if let Some(mt) = req.max_tokens {
        body.insert("max_tokens".into(), json!(mt));
    }
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        body.insert("top_p".into(), json!(p));
    }

    let url = format!("{}/chat/completions", route.base_url.trim_end_matches('/'));
    debug!("Calling upstream: {} with model {}", url, req.model);

    let mut request = client.post(&url).json(&body);
    if let Some(ref key) = route.api_key {
        request = request.header("Authorization", format!("Bearer {}", key));
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Upstream returned {}: {:.500}", status, body));
    }

    let json: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse upstream JSON: {}", e))?;

    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let finish_reason = json["choices"][0]["finish_reason"]
        .as_str()
        .map(String::from);
    let usage = &json["usage"];

    Ok(LlmResponse {
        content,
        finish_reason,
        prompt_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0) as u32,
        completion_tokens: usage["completion_tokens"].as_u64().unwrap_or(0) as u32,
        total_tokens: usage["total_tokens"].as_u64().unwrap_or(0) as u32,
    })
}

/// Approximate cost in cents based on per-1M-token pricing.
fn estimate_cost(provider: &str, tokens_in: u64, tokens_out: u64) -> u64 {
    // Prices in USD per 1M tokens (approximate, as of mid-2025)
    let (price_in, price_out): (f64, f64) = match provider {
        "deepseek" => (0.27, 1.10),
        "openai" => (2.50, 10.00),
        "anthropic" => (3.00, 15.00),
        "openrouter" => (2.00, 8.00),
        "xai" => (2.00, 8.00),
        "google" => (1.25, 5.00),
        _ => (1.00, 4.00),
    };
    // Convert from USD-per-1M-tokens to cents
    let cost = (tokens_in as f64 * price_in + tokens_out as f64 * price_out) / 10_000.0;
    cost.ceil() as u64
}

/// Build a JSON error response with the proper status code.
fn json_error(
    status: StatusCode,
    type_: &str,
    message: &str,
) -> (StatusCode, Json<Value>) {
    let body = json!({
        "error": {
            "message": message,
            "type": type_,
            "code": status.as_u16(),
        }
    });
    (status, Json(body))
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brokering::BrokeringConfig;
    use crate::brokering::TenantManager;
    use tempfile::TempDir;

    fn setup_db() -> (String, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("api_test.db");
        (path.to_string_lossy().to_string(), dir)
    }

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(&[]), 100);
    }

    #[test]
    fn test_estimate_tokens_with_content() {
        let msgs = vec![json!({"role": "user", "content": "Hello world!"})];
        let tokens = estimate_tokens(&msgs);
        assert!(tokens > 100); // 12 chars / 4 + 100
    }

    #[test]
    fn test_find_tenant_by_key_not_found() {
        let (path, _dir) = setup_db();
        let tm = TenantManager::new(&path).unwrap();
        let err = find_tenant_by_key(&tm, "nonexistent-key");
        assert!(err.is_err());
    }

    #[test]
    fn test_extract_bearer_token() {
        let mut headers = HeaderMap::new();
        assert!(extract_bearer_token(&headers).is_none());

        headers.insert("authorization", "Bearer abc123".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), Some("abc123".into()));

        // Wrong prefix
        headers.insert("authorization", "Basic abc123".parse().unwrap());
        assert!(extract_bearer_token(&headers).is_none());

        // Empty token
        headers.insert("authorization", "Bearer ".parse().unwrap());
        assert!(extract_bearer_token(&headers).is_none());
    }

    #[test]
    fn test_estimate_cost_known_providers() {
        let cost = estimate_cost("deepseek", 1_000_000, 1_000_000);
        // 1M in * 0.27 + 1M out * 1.10 = 1.37 USD = 137 cents, / 10000 scaling
        // Actually: (1M * 0.27 + 1M * 1.10) / 10000 = (270000 + 1100000) / 10000 = 137 cents
        assert!(cost > 0);

        let cost = estimate_cost("openai", 1000, 1000);
        // (1000 * 2.50 + 1000 * 10.00) / 10000 = 12500 / 10000 = 1.25 -> ceil to 2
        assert_eq!(cost, 2);
    }
}
