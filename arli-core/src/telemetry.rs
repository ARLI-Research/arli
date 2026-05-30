//! Structured telemetry — JSON observability with span context.
//!
//! Initializes JSON-structured logging with span context.
//! All `tracing::*` macros carry span IDs and timing info.
//!
//! Ready for OTel collector export via feature flag in the future.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

/// Initialize structured JSON tracing.
///
/// Outputs JSON logs with:
/// - timestamp, level, target, span name, span fields
/// - parent span IDs for call chain reconstruction
pub fn init_tracing(service_name: Option<&str>) {
    let _service = service_name.unwrap_or("arli");

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,arli_core=debug"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_target(true)
        .with_span_list(true)
        .with_current_span(true);

    Registry::default()
        .with(filter)
        .with(fmt_layer)
        .init();

    tracing::info!(
        service = _service,
        "Tracing initialized — structured JSON output enabled"
    );
}

/// Create an instrumented span for agent operations.
///
/// Returns a guard that must be held for the span's duration.
#[macro_export]
macro_rules! agent_span {
    ($name:expr, $($key:ident = $value:expr),* $(,)?) => {{
        let _span = tracing::info_span!($name, $($key = $value),*);
        let _guard = _span.enter();
        _guard
    }};
}
