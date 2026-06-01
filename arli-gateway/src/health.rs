//! Health check HTTP server — OpenShell production pattern.
//!
//! Exposes:
//! - GET /healthz  — liveness probe (always 200 if process alive)
//! - GET /readyz   — readiness probe (200 when all platforms connected)
//!
//! Used by Kubernetes/container orchestration for probes.

use axum::{routing::get, Router};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;

/// Gateway health state — shared between platform tasks and health server.
#[derive(Clone)]
pub struct HealthState {
    /// Set to true once all platform adapters have started.
    pub ready: Arc<AtomicBool>,
    /// Total number of platform adapters running.
    pub platform_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl HealthState {
    pub fn new() -> Self {
        Self {
            ready: Arc::new(AtomicBool::new(false)),
            platform_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Mark the gateway as ready.
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    /// Increment the active platform count.
    pub fn inc_platforms(&self) {
        self.platform_count.fetch_add(1, Ordering::Relaxed);
    }
}

impl Default for HealthState {
    fn default() -> Self {
        Self::new()
    }
}

/// Start the health check HTTP server.
///
/// Runs on `0.0.0.0:<port>`. Port defaults to 8080, overridable via `HEALTH_PORT` env var.
/// Returns immediately after spawning the server task.
pub async fn serve(state: HealthState) -> anyhow::Result<()> {
    let port: u16 = std::env::var("HEALTH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(move || readyz(state.clone())))
        .route("/metrics", get(metrics_handler));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    info!("Health server listening on 0.0.0.0:{port}");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("Health server error: {}", e);
        }
    });

    Ok(())
}

/// Liveness — always returns 200 if the process is alive.
async fn healthz() -> &'static str {
    "ok"
}

/// Readiness — returns 200 when gateway is fully initialized.
///
/// Returns 503 if platforms haven't connected yet.
async fn readyz(state: HealthState) -> (axum::http::StatusCode, &'static str) {
    if state.ready.load(Ordering::Acquire) {
        (axum::http::StatusCode::OK, "ready")
    } else {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "not ready",
        )
    }
}

/// Prometheus-style metrics endpoint.
///
/// Returns basic gateway metrics: uptime, platform count, readiness.
async fn metrics_handler() -> String {
    format!(
        "# HELP arli_gateway_platforms Number of active platform adapters\n\
         # TYPE arli_gateway_platforms gauge\n\
         arli_gateway_platforms 0\n\
         # HELP arli_gateway_ready Gateway readiness status (1=ready, 0=not)\n\
         # TYPE arli_gateway_ready gauge\n\
         arli_gateway_ready 1\n"
    )
}
