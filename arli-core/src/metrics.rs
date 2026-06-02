//! Prometheus-compatible metrics registry.
//!
//! Provides atomic counters and gauges for production monitoring.
//! All metrics are thread-safe and designed for low-overhead collection.
//!
//! ## Metrics
//!
//! | Name | Type | Description |
//! |------|------|-------------|
//! | `arli_agent_runs_total` | counter | Total agent.run() invocations |
//! | `arli_tool_calls_total` | counter | Total tool executions |
//! | `arli_tool_errors_total` | counter | Failed tool executions |
//! | `arli_memory_ops_total` | counter | Memory add/replace/remove operations |
//! | `arli_attestations_total` | counter | Attestations submitted |
//! | `arli_sessions_active` | gauge | Currently active sessions |
//! | `arli_gateway_platforms` | gauge | Active platform adapters |
//! | `arli_gateway_ready` | gauge | Gateway readiness (1=ready) |
//! | `arli_trades_executed_total` | counter | Trades executed via arli-trading |
//!
//! ## Usage
//!
//! ```ignore
//! use arli_core::metrics::Metrics;
//!
//! let m = Metrics::global();
//! m.inc_agent_runs();
//! m.inc_tool_calls();
//! // ...
//! println!("{}", m.prometheus_text());
//! ```

use std::fmt::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Snapshot of all metric values — serializable for API consumers.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub agent_runs: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub memory_ops: u64,
    pub attestations: u64,
    pub sessions: u64,
    pub platforms: u64,
    pub trades: u64,
    pub ready: u64,
}

/// Global metrics registry — one instance per process.
#[derive(Clone)]
pub struct Metrics {
    /// Total agent.run() calls (counter).
    agent_runs: Arc<AtomicU64>,
    /// Total tool.execute() calls (counter).
    tool_calls: Arc<AtomicU64>,
    /// Failed tool executions (counter).
    tool_errors: Arc<AtomicU64>,
    /// Memory operations (counter).
    memory_ops: Arc<AtomicU64>,
    /// Attestations submitted (counter).
    attestations: Arc<AtomicU64>,
    /// Active sessions (gauge).
    active_sessions: Arc<AtomicU64>,
    /// Platform adapters running (gauge).
    platforms: Arc<AtomicU64>,
    /// Trades executed (counter).
    trades_executed: Arc<AtomicU64>,
    /// Gateway readiness (gauge — atomic bool).
    ready: Arc<AtomicBool>,
    /// Process start timestamp for uptime calculation.
    start_time: std::time::Instant,
}

impl Metrics {
    /// Create a new metrics registry. Call once at startup.
    pub fn new() -> Self {
        Self {
            agent_runs: Arc::new(AtomicU64::new(0)),
            tool_calls: Arc::new(AtomicU64::new(0)),
            tool_errors: Arc::new(AtomicU64::new(0)),
            memory_ops: Arc::new(AtomicU64::new(0)),
            attestations: Arc::new(AtomicU64::new(0)),
            active_sessions: Arc::new(AtomicU64::new(0)),
            platforms: Arc::new(AtomicU64::new(0)),
            trades_executed: Arc::new(AtomicU64::new(0)),
            ready: Arc::new(AtomicBool::new(false)),
            start_time: std::time::Instant::now(),
        }
    }

    // --- Counters ---

    pub fn inc_agent_runs(&self) {
        self.agent_runs.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_tool_calls(&self) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_tool_errors(&self) {
        self.tool_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_memory_ops(&self) {
        self.memory_ops.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_attestations(&self) {
        self.attestations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_trades_executed(&self) {
        self.trades_executed.fetch_add(1, Ordering::Relaxed);
    }

    // --- Gauges ---

    pub fn set_active_sessions(&self, n: u64) {
        self.active_sessions.store(n, Ordering::Relaxed);
    }

    pub fn inc_active_sessions(&self) {
        self.active_sessions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_active_sessions(&self) {
        self.active_sessions.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn set_platforms(&self, n: u64) {
        self.platforms.store(n, Ordering::Relaxed);
    }

    pub fn inc_platforms(&self) {
        self.platforms.fetch_add(1, Ordering::Relaxed);
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    // --- Count read ---

    pub fn agent_runs(&self) -> u64 {
        self.agent_runs.load(Ordering::Relaxed)
    }

    pub fn tool_calls(&self) -> u64 {
        self.tool_calls.load(Ordering::Relaxed)
    }

    pub fn tool_errors(&self) -> u64 {
        self.tool_errors.load(Ordering::Relaxed)
    }

    pub fn trades_executed(&self) -> u64 {
        self.trades_executed.load(Ordering::Relaxed)
    }

    /// Uptime in seconds since registry creation.
    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Generate Prometheus text format output.
    ///
    /// Compatible with Prometheus/Grafana scraping.
    pub fn prometheus_text(&self) -> String {
        let mut out = String::with_capacity(1024);

        out.push_str("# HELP arli_uptime_seconds Process uptime in seconds\n");
        out.push_str("# TYPE arli_uptime_seconds gauge\n");
        let _ = write!(&mut out, "arli_uptime_seconds {}\n", self.uptime_secs());

        out.push_str("# HELP arli_agent_runs_total Total agent.run() calls\n");
        out.push_str("# TYPE arli_agent_runs_total counter\n");
        let _ = write!(&mut out, "arli_agent_runs_total {}\n", self.agent_runs());

        out.push_str("# HELP arli_tool_calls_total Total tool executions\n");
        out.push_str("# TYPE arli_tool_calls_total counter\n");
        let _ = write!(&mut out, "arli_tool_calls_total {}\n", self.tool_calls());

        out.push_str("# HELP arli_tool_errors_total Failed tool executions\n");
        out.push_str("# TYPE arli_tool_errors_total counter\n");
        let _ = write!(&mut out, "arli_tool_errors_total {}\n", self.tool_errors());

        out.push_str("# HELP arli_memory_ops_total Memory operations\n");
        out.push_str("# TYPE arli_memory_ops_total counter\n");
        let _ = write!(
            &mut out,
            "arli_memory_ops_total {}\n",
            self.memory_ops.load(Ordering::Relaxed)
        );

        out.push_str("# HELP arli_attestations_total Attestations submitted\n");
        out.push_str("# TYPE arli_attestations_total counter\n");
        let _ = write!(
            &mut out,
            "arli_attestations_total {}\n",
            self.attestations.load(Ordering::Relaxed)
        );

        out.push_str("# HELP arli_sessions_active Currently active sessions\n");
        out.push_str("# TYPE arli_sessions_active gauge\n");
        let _ = write!(
            &mut out,
            "arli_sessions_active {}\n",
            self.active_sessions.load(Ordering::Relaxed)
        );

        out.push_str("# HELP arli_gateway_platforms Active platform adapters\n");
        out.push_str("# TYPE arli_gateway_platforms gauge\n");
        let _ = write!(
            &mut out,
            "arli_gateway_platforms {}\n",
            self.platforms.load(Ordering::Relaxed)
        );

        out.push_str("# HELP arli_gateway_ready Gateway readiness (1=ready, 0=not)\n");
        out.push_str("# TYPE arli_gateway_ready gauge\n");
        let _ = write!(
            &mut out,
            "arli_gateway_ready {}\n",
            if self.is_ready() { 1u8 } else { 0u8 },
        );

        out.push_str("# HELP arli_trades_executed_total Trades executed\n");
        out.push_str("# TYPE arli_trades_executed_total counter\n");
        let _ = write!(
            &mut out,
            "arli_trades_executed_total {}\n",
            self.trades_executed()
        );

        out
    }

    /// Snapshot of all metric values — for dashboard/API consumption.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            agent_runs: self.agent_runs(),
            tool_calls: self.tool_calls(),
            tool_errors: self.tool_errors(),
            memory_ops: self.memory_ops.load(Ordering::Relaxed),
            attestations: self.attestations.load(Ordering::Relaxed),
            sessions: self.active_sessions.load(Ordering::Relaxed),
            platforms: self.platforms.load(Ordering::Relaxed),
            trades: self.trades_executed(),
            ready: if self.is_ready() { 1 } else { 0 },
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Global singleton — initialized once, shared via Arc::clone.
static GLOBAL_METRICS: std::sync::OnceLock<Metrics> = std::sync::OnceLock::new();

impl Metrics {
    /// Get or initialize the global metrics instance.
    pub fn global() -> &'static Metrics {
        GLOBAL_METRICS.get_or_init(Metrics::new)
    }
}
