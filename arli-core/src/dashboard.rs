//! Web UI dashboard — axum + htmx, no JavaScript framework.
//!
//! Serves a lightweight dashboard at `/dashboard` with tabs for:
//! - Overview: system stats (agents, tools, kanban cards, metrics)
//! - Kanban: interactive board viewer
//! - Agents: agent list with status
//! - Metrics: prometheus-style raw metrics
//!
//! All dynamic content loaded via htmx partials. Clean, dark theme.
//! The dashboard is optional — gateway enables it via `DashboardConfig`.

use axum::{
    extract::State,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Serialize;
use std::sync::{Arc, RwLock};

use crate::kanban::KanbanStore;
use crate::metrics::Metrics;

// ── Config ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DashboardConfig {
    pub port: u16,
    pub title: String,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            title: "ARLI Dashboard".into(),
        }
    }
}

// ── Shared State ─────────────────────────────────────────────────────────

/// Metrics snapshot — serialized for JSON API.
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub agent_runs: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub memory_ops: u64,
    pub attestations: u64,
    pub sessions: u64,
    pub trades: u64,
    pub ready: u64,
}

/// Agent info for the dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KanbanSummary {
    pub total_boards: usize,
    pub total_cards: usize,
}

/// Shared state accessible by all dashboard handlers.
pub struct DashboardState {
    pub config: DashboardConfig,
    pub metrics: Arc<Metrics>,
    pub kanban: Option<Arc<KanbanStore>>,
    pub agents: RwLock<Vec<AgentInfo>>,
}

impl DashboardState {
    pub fn new(config: DashboardConfig, metrics: Arc<Metrics>) -> Self {
        Self {
            config,
            metrics,
            kanban: None,
            agents: RwLock::new(Vec::new()),
        }
    }

    pub fn with_kanban(mut self, kanban: Arc<KanbanStore>) -> Self {
        self.kanban = Some(kanban);
        self
    }
}

// ── Router Builder ───────────────────────────────────────────────────────

pub fn build_router(state: Arc<DashboardState>) -> Router {
    Router::new()
        // HTML pages
        .route("/dashboard", get(index_page))
        .route("/dashboard/overview", get(overview_partial))
        .route("/dashboard/kanban", get(kanban_page))
        .route("/dashboard/agents", get(agents_partial))
        .route("/dashboard/metrics", get(metrics_page))
        // JSON API
        .route("/api/stats", get(api_stats))
        .route("/api/kanban-summary", get(api_kanban_summary))
        .route("/api/agents", get(api_agents))
        .route("/api/metrics-raw", get(api_metrics_raw))
        // Static CSS
        .route("/dashboard/style.css", get(style_css))
        .with_state(state)
}

// ── HTML Templates ───────────────────────────────────────────────────────

const STYLE_CSS: &str = r#"
:root {
    --bg: #0f1117;
    --surface: #1a1b23;
    --border: #2a2b33;
    --text: #e1e1e6;
    --muted: #888899;
    --accent: #6366f1;
    --accent-hover: #818cf8;
    --green: #22c55e;
    --yellow: #eab308;
    --red: #ef4444;
    --radius: 8px;
}
* { margin:0; padding:0; box-sizing:border-box; }
body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    background: var(--bg); color: var(--text);
    min-height: 100vh;
    line-height: 1.5;
}
.container { max-width: 1200px; margin: 0 auto; padding: 24px; }
header { 
    background: var(--surface); border-bottom: 1px solid var(--border);
    padding: 16px 24px; display: flex; align-items: center; gap: 24px;
}
header h1 { font-size: 20px; font-weight: 600; color: var(--accent); }
nav a {
    color: var(--muted); text-decoration: none;
    padding: 6px 14px; border-radius: var(--radius);
    font-size: 14px; font-weight: 500;
    transition: all 0.15s;
}
nav a:hover, nav a.active { color: var(--text); background: var(--border); }
.card {
    background: var(--surface); border: 1px solid var(--border);
    border-radius: var(--radius); padding: 20px; margin-bottom: 16px;
}
.card h3 { font-size: 14px; color: var(--muted); margin-bottom: 8px; text-transform: uppercase; letter-spacing: 0.05em; }
.card .value { font-size: 32px; font-weight: 700; }
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 16px; }
.stat-green .value { color: var(--green); }
.stat-yellow .value { color: var(--yellow); }
.stat-red .value { color: var(--red); }
.badge {
    display: inline-block; padding: 2px 8px; border-radius: 4px;
    font-size: 11px; font-weight: 600; text-transform: uppercase;
}
.badge-ok { background: #064e3b; color: var(--green); }
.badge-warn { background: #713f12; color: var(--yellow); }
.badge-err { background: #7f1d1d; color: var(--red); }
table { width: 100%; border-collapse: collapse; }
th, td { padding: 10px 14px; text-align: left; border-bottom: 1px solid var(--border); font-size: 14px; }
th { color: var(--muted); font-weight: 600; font-size: 12px; text-transform: uppercase; }
tr:hover { background: var(--border); }
.metrics-raw { 
    background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius);
    padding: 20px; font-family: 'SF Mono', 'Fira Code', monospace; font-size: 13px;
    white-space: pre-wrap; line-height: 1.7; color: var(--accent-hover);
}
.kanban-column {
    background: var(--surface); border: 1px solid var(--border);
    border-radius: var(--radius); padding: 12px; min-width: 200px;
}
.kanban-column h4 {
    font-size: 12px; text-transform: uppercase; letter-spacing: 0.05em;
    color: var(--muted); margin-bottom: 10px; padding-bottom: 8px;
    border-bottom: 2px solid var(--accent);
}
.kanban-card {
    background: var(--bg); border: 1px solid var(--border); border-radius: 4px;
    padding: 10px; margin-bottom: 8px; font-size: 13px;
}
.kanban-card .prio {
    font-size: 10px; font-weight: 700; text-transform: uppercase;
}
.prio-critical { color: var(--red); }
.prio-high { color: var(--yellow); }
.prio-medium { color: var(--muted); }
.prio-low { color: var(--muted); }
.kanban-grid { display: flex; gap: 16px; overflow-x: auto; padding-bottom: 12px; }
.flex-between { display: flex; justify-content: space-between; align-items: center; }
.text-muted { color: var(--muted); font-size: 13px; }
.mt-8 { margin-top: 8px; }
.mb-16 { margin-bottom: 16px; }
"#;

const INDEX_HTML: &str = r###"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>ARLI Dashboard</title>
    <link rel="stylesheet" href="/dashboard/style.css">
    <script src="https://unpkg.com/htmx.org@2.0.4"></script>
</head>
<body>
    <header>
        <h1>ARLI</h1>
        <nav hx-boost="true">
            <a href="/dashboard/overview" hx-target="#content" hx-push-url="true" class="active">Overview</a>
            <a href="/dashboard/kanban" hx-target="#content" hx-push-url="true">Kanban</a>
            <a href="/dashboard/agents" hx-target="#content" hx-push-url="true">Agents</a>
            <a href="/dashboard/metrics" hx-target="#content" hx-push-url="true">Metrics</a>
        </nav>
    </header>
    <div class="container" id="content" hx-get="/dashboard/overview" hx-trigger="load"></div>
</body>
</html>"###;

const OVERVIEW_HTML: &str = r#"<div>
    <h2 class="mb-16">System Overview</h2>
    <div class="grid" id="stat-grid">
        <div class="card">
            <h3>Agent Runs</h3>
            <div class="value">{{agent_runs}}</div>
        </div>
        <div class="card">
            <h3>Tool Calls</h3>
            <div class="value">{{tool_calls}}</div>
        </div>
        <div class="card stat-red">
            <h3>Errors</h3>
            <div class="value">{{tool_errors}}</div>
        </div>
        <div class="card">
            <h3>Memory Ops</h3>
            <div class="value">{{memory_ops}}</div>
        </div>
        <div class="card">
            <h3>Attestations</h3>
            <div class="value">{{attestations}}</div>
        </div>
        <div class="card">
            <h3>Sessions</h3>
            <div class="value">{{sessions}}</div>
        </div>
        <div class="card">
            <h3>Trades</h3>
            <div class="value">{{trades}}</div>
        </div>
        <div class="card">
            <h3>Kanban Boards</h3>
            <div class="value">{{kanban_boards}}</div>
        </div>
        <div class="card">
            <h3>Kanban Cards</h3>
            <div class="value">{{kanban_cards}}</div>
        </div>
    </div>
    <div class="card mt-8">
        <h3>Status</h3>
        <div class="flex-between">
            <span>System</span>
            <span class="badge badge-ok">Ready</span>
        </div>
    </div>
</div>"#;

const AGENTS_HTML_PREFIX: &str = r#"<div>
    <h2 class="mb-16">Agents</h2>
    <table>
        <thead><tr><th>ID</th><th>Name</th><th>Status</th></tr></thead>
        <tbody>"#;

const AGENTS_HTML_SUFFIX: &str = r#"</tbody></table></div>"#;

const AGENT_ROW: &str = r#"<tr><td>{{id}}</td><td>{{name}}</td><td><span class="badge badge-ok">{{status}}</span></td></tr>"#;

const KANBAN_PAGE_PREFIX: &str = r#"<div>
    <h2 class="mb-16">Kanban Boards</h2>
    <div class="mb-16 text-muted" id="kanban-board-selector">
        {{board_selector}}
    </div>
    <div id="kanban-content">"#;

const KANBAN_PAGE_SUFFIX: &str = r#"</div></div>"#;

const METRICS_PAGE_PREFIX: &str = r#"<div>
    <h2 class="mb-16">Raw Metrics</h2>
    <div class="metrics-raw">"#;

const METRICS_PAGE_SUFFIX: &str = r#"</div></div>"#;

const NO_KANBAN: &str = r#"<p class="text-muted">No kanban boards configured. Create one with the <code>kanban</code> tool.</p>"#;

// ── Handlers ─────────────────────────────────────────────────────────────

/// Main dashboard page (full HTML).
async fn index_page() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// Overview partial — replaces #content.
async fn overview_partial(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let metrics = state.metrics.snapshot();
    let kanban_summary = get_kanban_summary(&state);

    let html = OVERVIEW_HTML
        .replace("{{agent_runs}}", &metrics.agent_runs.to_string())
        .replace("{{tool_calls}}", &metrics.tool_calls.to_string())
        .replace("{{tool_errors}}", &metrics.tool_errors.to_string())
        .replace("{{memory_ops}}", &metrics.memory_ops.to_string())
        .replace("{{attestations}}", &metrics.attestations.to_string())
        .replace("{{sessions}}", &metrics.sessions.to_string())
        .replace("{{trades}}", &metrics.trades.to_string())
        .replace(
            "{{kanban_boards}}",
            &kanban_summary.total_boards.to_string(),
        )
        .replace("{{kanban_cards}}", &kanban_summary.total_cards.to_string());

    Html(html)
}

/// Kanban page — shows board selector + columns.
async fn kanban_page(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let kanban = match &state.kanban {
        Some(k) => k,
        None => {
            return Html(format!(
                "{KANBAN_PAGE_PREFIX}{NO_KANBAN}{KANBAN_PAGE_SUFFIX}"
            ))
        }
    };

    // Build board selector dropdown
    let boards = kanban.list_boards().unwrap_or_default();
    if boards.is_empty() {
        return Html(format!(
            "{KANBAN_PAGE_PREFIX}{NO_KANBAN}{KANBAN_PAGE_SUFFIX}"
        ));
    }

    // Use first board
    let board = &boards[0];
    let stats = kanban
        .get_board_stats(&board.id)
        .unwrap_or_else(|_| crate::kanban::BoardStats {
            board_id: board.id.clone(),
            board_name: board.name.clone(),
            total_cards: 0,
            columns: Vec::new(),
        });

    let mut html = String::from(KANBAN_PAGE_PREFIX);

    // Board selector (simple text for now)
    html.push_str(&format!(
        "<span class=\"badge badge-ok\">{}</span>",
        board.name
    ));
    html.push_str("</div><div id=\"kanban-content\">");

    // Column layout
    html.push_str(r#"<div class="kanban-grid">"#);
    for col in &stats.columns {
        html.push_str(&format!(
            r#"<div class="kanban-column"><h4>{} ({})</h4>"#,
            col.column_name, col.card_count
        ));
        for card in &col.cards {
            let prio_class = match card.priority {
                crate::kanban::Priority::Critical => "prio-critical",
                crate::kanban::Priority::High => "prio-high",
                crate::kanban::Priority::Medium => "prio-medium",
                crate::kanban::Priority::Low => "prio-low",
            };
            html.push_str(&format!(
                r#"<div class="kanban-card"><span class="prio {}">{}</span><br>{}"#,
                prio_class,
                card.priority.as_str(),
                html_escape(&card.title),
            ));
            if let Some(ref assignee) = card.assignee {
                html.push_str(&format!(
                    r#"<br><small class="text-muted">{}</small>"#,
                    assignee
                ));
            }
            html.push_str("</div>");
        }
        html.push_str("</div>");
    }
    html.push_str("</div>");

    html.push_str(KANBAN_PAGE_SUFFIX);
    Html(html)
}

/// Agents tab — table of registered agents.
async fn agents_partial(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let agents = state.agents.read().unwrap();
    let mut html = String::from(AGENTS_HTML_PREFIX);

    if agents.is_empty() {
        html.push_str(r#"<tr><td colspan="3" class="text-muted">No agents registered</td></tr>"#);
    } else {
        for agent in agents.iter() {
            html.push_str(
                &AGENT_ROW
                    .replace("{{id}}", &agent.id)
                    .replace("{{name}}", &agent.name)
                    .replace("{{status}}", &agent.status),
            );
        }
    }

    html.push_str(AGENTS_HTML_SUFFIX);
    Html(html)
}

/// Metrics page — raw prometheus format.
async fn metrics_page(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let metrics = state.metrics.snapshot();
    let raw = format!(
        "arli_agent_runs_total {}\n\
         arli_tool_calls_total {}\n\
         arli_tool_errors_total {}\n\
         arli_memory_ops_total {}\n\
         arli_attestations_total {}\n\
         arli_sessions_total {}\n\
         arli_trades_total {}\n\
         arli_ready {}\n",
        metrics.agent_runs,
        metrics.tool_calls,
        metrics.tool_errors,
        metrics.memory_ops,
        metrics.attestations,
        metrics.sessions,
        metrics.trades,
        metrics.ready,
    );
    let html = format!("{METRICS_PAGE_PREFIX}{raw}{METRICS_PAGE_SUFFIX}");
    Html(html)
}

/// JSON API: system stats.
async fn api_stats(State(state): State<Arc<DashboardState>>) -> Json<serde_json::Value> {
    let metrics = state.metrics.snapshot();
    let kanban_summary = get_kanban_summary(&state);
    Json(serde_json::json!({
        "metrics": {
            "agent_runs": metrics.agent_runs,
            "tool_calls": metrics.tool_calls,
            "tool_errors": metrics.tool_errors,
            "memory_ops": metrics.memory_ops,
            "attestations": metrics.attestations,
            "sessions": metrics.sessions,
            "trades": metrics.trades,
        },
        "kanban": {
            "boards": kanban_summary.total_boards,
            "cards": kanban_summary.total_cards,
        }
    }))
}

/// JSON API: kanban summary.
async fn api_kanban_summary(State(state): State<Arc<DashboardState>>) -> Json<serde_json::Value> {
    let summary = get_kanban_summary(&state);
    Json(serde_json::json!({
        "boards": summary.total_boards,
        "cards": summary.total_cards,
    }))
}

/// JSON API: agent list.
async fn api_agents(State(state): State<Arc<DashboardState>>) -> Json<Vec<AgentInfo>> {
    let agents = state.agents.read().unwrap();
    Json(agents.clone())
}

/// JSON API: raw metrics in prometheus format.
async fn api_metrics_raw(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let metrics = state.metrics.snapshot();
    let body = format!(
        "# HELP arli_agent_runs_total Total agent runs\n\
         # TYPE arli_agent_runs_total counter\n\
         arli_agent_runs_total {}\n\
         # HELP arli_tool_calls_total Total tool calls\n\
         # TYPE arli_tool_calls_total counter\n\
         arli_tool_calls_total {}\n\
         # HELP arli_tool_errors_total Total tool errors\n\
         # TYPE arli_tool_errors_total counter\n\
         arli_tool_errors_total {}\n\
         # HELP arli_memory_ops_total Total memory operations\n\
         # TYPE arli_memory_ops_total counter\n\
         arli_memory_ops_total {}\n\
         # HELP arli_attestations_total Total attestations\n\
         # TYPE arli_attestations_total counter\n\
         arli_attestations_total {}\n\
         # HELP arli_sessions_total Total sessions\n\
         # TYPE arli_sessions_total counter\n\
         arli_sessions_total {}\n\
         # HELP arli_trades_total Total trades\n\
         # TYPE arli_trades_total counter\n\
         arli_trades_total {}\n\
         # HELP arli_ready Ready status (1=ready)\n\
         # TYPE arli_ready gauge\n\
         arli_ready {}\n",
        metrics.agent_runs,
        metrics.tool_calls,
        metrics.tool_errors,
        metrics.memory_ops,
        metrics.attestations,
        metrics.sessions,
        metrics.trades,
        metrics.ready,
    );
    ([("content-type", "text/plain; charset=utf-8")], body)
}

/// CSS stylesheet.
async fn style_css() -> impl IntoResponse {
    ([("content-type", "text/css; charset=utf-8")], STYLE_CSS)
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn get_kanban_summary(state: &DashboardState) -> KanbanSummary {
    match &state.kanban {
        Some(store) => {
            let boards = store.list_boards().unwrap_or_default();
            let total_cards: usize = boards
                .iter()
                .filter_map(|b| {
                    store
                        .list_cards(Some(&b.id), None, None)
                        .ok()
                        .map(|c| c.len())
                })
                .sum();
            KanbanSummary {
                total_boards: boards.len(),
                total_cards,
            }
        }
        None => KanbanSummary {
            total_boards: 0,
            total_cards: 0,
        },
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::KanbanStore;
    use crate::metrics::Metrics;

    fn test_state() -> Arc<DashboardState> {
        let config = DashboardConfig::default();
        let metrics = Arc::new(Metrics::new());
        let kanban = Arc::new(KanbanStore::open_in_memory().unwrap());
        let state = DashboardState::new(config, metrics).with_kanban(kanban);
        Arc::new(state)
    }

    #[test]
    fn test_metrics_snapshot_defaults() {
        let metrics = Metrics::new();
        let snap = metrics.snapshot();
        assert_eq!(snap.agent_runs, 0);
        assert_eq!(snap.tool_calls, 0);
    }

    #[test]
    fn test_kanban_summary_empty() {
        let state = test_state();
        let summary = get_kanban_summary(&state);
        assert_eq!(summary.total_boards, 0);
        assert_eq!(summary.total_cards, 0);
    }

    #[test]
    fn test_kanban_summary_with_data() {
        let state = test_state();
        let store = state.kanban.as_ref().unwrap();
        let board = store.create_board("Test", "").unwrap();
        let cols = store.list_columns(&board.id).unwrap();
        store
            .add_card(
                &board.id,
                &cols[0].id,
                "Task",
                "",
                crate::kanban::Priority::Medium,
                None,
                &[],
                None,
            )
            .unwrap();

        let summary = get_kanban_summary(&state);
        assert_eq!(summary.total_boards, 1);
        assert_eq!(summary.total_cards, 1);
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
    }

    #[test]
    fn test_overview_page_renders() {
        let state = test_state();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let html = rt.block_on(overview_partial(State(state)));
        assert!(html.0.contains("System Overview"));
        assert!(html.0.contains("Agent Runs"));
        assert!(html.0.contains("Kanban Boards"));
    }

    #[test]
    fn test_kanban_page_renders_with_data() {
        let state = test_state();
        let store = state.kanban.as_ref().unwrap();
        let board = store.create_board("Sprint", "").unwrap();
        let cols = store.list_columns(&board.id).unwrap();
        store
            .add_card(
                &board.id,
                &cols[0].id,
                "Fix bug",
                "",
                crate::kanban::Priority::Critical,
                Some("agent-1"),
                &[],
                None,
            )
            .unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let html = rt.block_on(kanban_page(State(state)));
        assert!(html.0.contains("Sprint"));
        assert!(html.0.contains("Fix bug"));
        assert!(html.0.contains("critical"));
    }

    #[test]
    fn test_kanban_page_empty() {
        let config = DashboardConfig::default();
        let metrics = Arc::new(Metrics::new());
        let state = DashboardState::new(config, metrics);
        let state = Arc::new(state);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let html = rt.block_on(kanban_page(State(state)));
        assert!(html.0.contains("No kanban boards"));
    }

    #[test]
    fn test_agents_page_empty() {
        let state = test_state();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let html = rt.block_on(agents_partial(State(state)));
        assert!(html.0.contains("No agents registered"));
    }

    #[test]
    fn test_agents_page_with_data() {
        let state = test_state();
        {
            let mut agents = state.agents.write().unwrap();
            agents.push(AgentInfo {
                id: "a1".into(),
                name: "Trader".into(),
                status: "running".into(),
            });
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        let html = rt.block_on(agents_partial(State(Arc::clone(&state))));
        assert!(html.0.contains("Trader"));
        assert!(html.0.contains("running"));
    }

    #[test]
    fn test_api_stats_json() {
        let state = test_state();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let json = rt.block_on(api_stats(State(state)));
        assert!(json["metrics"]["agent_runs"].as_u64().is_some());
        assert!(json["kanban"]["boards"].as_u64().is_some());
    }
}
