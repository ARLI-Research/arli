//! Lifecycle hooks — extensibility points for plugins and custom behavior.
//!
//! Hooks fire at specific points in the agent's lifecycle, allowing
//! external code to intercept tool calls, react to errors, and track progress.
//!
//! Design:
//! - Each hook is an async function that receives context and can mutate it
//! - Hooks are registered globally in the HookRegistry
//! - PreToolCall hooks can DENY a tool execution (last hook wins)
//! - PostToolCall hooks receive the result for logging/metrics

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::tools::ToolOutput;

// ── Context types ──

/// Context passed to PreToolCall hooks.
#[derive(Debug, Clone)]
pub struct PreToolCallContext {
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    pub agent_name: String,
    pub session_id: Option<String>,
}

/// Context passed to PostToolCall hooks.
#[derive(Debug, Clone)]
pub struct PostToolCallContext {
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    pub result: ToolOutput,
    pub elapsed_ms: u64,
    pub agent_name: String,
    pub session_id: Option<String>,
}

/// Context passed to session lifecycle hooks.
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub session_id: String,
    pub agent_name: String,
}

/// Context passed to compaction hooks.
#[derive(Debug, Clone)]
pub struct CompactionContext {
    pub session_id: String,
    pub agent_name: String,
    pub compacted_count: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub summary: String,
}

/// Context passed to error hooks.
#[derive(Debug, Clone)]
pub struct ErrorContext {
    pub agent_name: String,
    pub session_id: Option<String>,
    pub error: String,
    pub phase: String, // e.g. "tool_call", "llm_call", "compaction"
}

// ── Decision enum ──

/// Result of a PreToolCall hook: should the tool execute?
#[derive(Debug, Clone, PartialEq)]
pub enum HookDecision {
    /// Allow the tool to execute (default if no hooks deny it)
    Allow,
    /// Block the tool with a reason
    Deny(String),
}

// ── Hook trait ──

/// A lifecycle hook that can be registered with the agent.
///
/// All methods have default no-op implementations — only override what you need.
#[async_trait]
pub trait Hook: Send + Sync {
    /// Unique name for this hook (for debugging/logging).
    fn name(&self) -> &str;

    /// Called BEFORE a tool executes. Return Deny to block it.
    async fn pre_tool_call(&self, _ctx: &PreToolCallContext) -> HookDecision {
        HookDecision::Allow
    }

    /// Called AFTER a tool executes (receives the result).
    async fn post_tool_call(&self, _ctx: &PostToolCallContext) {}

    /// Called when a new session starts.
    async fn on_session_start(&self, _ctx: &SessionContext) {}

    /// Called when a session ends (completed, failed, or stopped).
    async fn on_session_end(&self, _ctx: &SessionContext) {}

    /// Called after compaction completes.
    async fn on_compaction(&self, _ctx: &CompactionContext) {}

    /// Called when an error occurs (tool failure, LLM error, etc.)
    async fn on_error(&self, _ctx: &ErrorContext) {}
}

// ── Registry ──

/// Registry of lifecycle hooks.
///
/// Thread-safe — can be shared across agents via Arc.
#[derive(Default)]
pub struct HookRegistry {
    hooks: RwLock<Vec<Box<dyn Hook>>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            hooks: RwLock::new(Vec::new()),
        }
    }

    /// Register a hook.
    pub async fn register(&self, hook: Box<dyn Hook>) {
        tracing::info!("Registered hook: {}", hook.name());
        self.hooks.write().await.push(hook);
    }

    /// Run all pre-tool-call hooks. Returns Deny if any hook blocks.
    /// If multiple hooks deny, the LAST denial wins (for logging).
    pub async fn run_pre_tool_call(&self, ctx: &PreToolCallContext) -> HookDecision {
        let hooks = self.hooks.read().await;
        let mut decision = HookDecision::Allow;

        for hook in hooks.iter() {
            let d = hook.pre_tool_call(ctx).await;
            if let HookDecision::Deny(ref reason) = d {
                tracing::warn!(
                    "Hook '{}' denied tool '{}': {}",
                    hook.name(),
                    ctx.tool_name,
                    reason
                );
                decision = d;
            }
        }

        decision
    }

    /// Run all post-tool-call hooks.
    pub async fn run_post_tool_call(&self, ctx: &PostToolCallContext) {
        let hooks = self.hooks.read().await;
        for hook in hooks.iter() {
            hook.post_tool_call(ctx).await;
        }
    }

    /// Run session-start hooks.
    pub async fn run_on_session_start(&self, ctx: &SessionContext) {
        let hooks = self.hooks.read().await;
        for hook in hooks.iter() {
            hook.on_session_start(ctx).await;
        }
    }

    /// Run session-end hooks.
    pub async fn run_on_session_end(&self, ctx: &SessionContext) {
        let hooks = self.hooks.read().await;
        for hook in hooks.iter() {
            hook.on_session_end(ctx).await;
        }
    }

    /// Run compaction hooks.
    pub async fn run_on_compaction(&self, ctx: &CompactionContext) {
        let hooks = self.hooks.read().await;
        for hook in hooks.iter() {
            hook.on_compaction(ctx).await;
        }
    }

    /// Run error hooks.
    pub async fn run_on_error(&self, ctx: &ErrorContext) {
        let hooks = self.hooks.read().await;
        for hook in hooks.iter() {
            hook.on_error(ctx).await;
        }
    }

    pub async fn count(&self) -> usize {
        self.hooks.read().await.len()
    }
}

// ── Built-in example hooks ──

/// A hook that logs every tool call and its result.
pub struct LoggingHook;

#[async_trait]
impl Hook for LoggingHook {
    fn name(&self) -> &str { "logging" }

    async fn pre_tool_call(&self, ctx: &PreToolCallContext) -> HookDecision {
        tracing::info!(
            "Tool '{}' called by '{}' (session: {:?})",
            ctx.tool_name,
            ctx.agent_name,
            ctx.session_id
        );
        HookDecision::Allow
    }

    async fn post_tool_call(&self, ctx: &PostToolCallContext) {
        tracing::info!(
            "Tool '{}' completed in {}ms (success={})",
            ctx.tool_name,
            ctx.elapsed_ms,
            ctx.result.success
        );
    }

    async fn on_error(&self, ctx: &ErrorContext) {
        tracing::error!(
            "Agent '{}' error in phase '{}': {}",
            ctx.agent_name,
            ctx.phase,
            ctx.error
        );
    }
}

/// A hook that counts metrics (placeholder for real metrics hook).
pub struct MetricsHook {
    tool_calls: tokio::sync::Mutex<u64>,
    errors: tokio::sync::Mutex<u64>,
    compactions: tokio::sync::Mutex<u64>,
}

impl Default for MetricsHook {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsHook {
    pub fn new() -> Self {
        Self {
            tool_calls: tokio::sync::Mutex::new(0),
            errors: tokio::sync::Mutex::new(0),
            compactions: tokio::sync::Mutex::new(0),
        }
    }

    pub async fn tool_call_count(&self) -> u64 { *self.tool_calls.lock().await }
    pub async fn error_count(&self) -> u64 { *self.errors.lock().await }
    pub async fn compaction_count(&self) -> u64 { *self.compactions.lock().await }
}

#[async_trait]
impl Hook for MetricsHook {
    fn name(&self) -> &str { "metrics" }

    async fn post_tool_call(&self, _ctx: &PostToolCallContext) {
        *self.tool_calls.lock().await += 1;
    }

    async fn on_error(&self, _ctx: &ErrorContext) {
        *self.errors.lock().await += 1;
    }

    async fn on_compaction(&self, _ctx: &CompactionContext) {
        *self.compactions.lock().await += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DenyShellHook;
    #[async_trait]
    impl Hook for DenyShellHook {
        fn name(&self) -> &str { "deny-shell" }
        async fn pre_tool_call(&self, ctx: &PreToolCallContext) -> HookDecision {
            if ctx.tool_name == "shell" {
                HookDecision::Deny("Blocked by test hook".into())
            } else {
                HookDecision::Allow
            }
        }
    }

    #[tokio::test]
    async fn test_hook_denies_tool() {
        let registry = HookRegistry::new();
        registry.register(Box::new(DenyShellHook)).await;

        let decision = registry.run_pre_tool_call(&PreToolCallContext {
            tool_name: "shell".into(),
            tool_args: serde_json::json!({"cmd": "rm -rf /"}),
            agent_name: "test".into(),
            session_id: None,
        }).await;

        assert_eq!(decision, HookDecision::Deny("Blocked by test hook".into()));
    }

    #[tokio::test]
    async fn test_hook_allows_other_tools() {
        let registry = HookRegistry::new();
        registry.register(Box::new(DenyShellHook)).await;

        let decision = registry.run_pre_tool_call(&PreToolCallContext {
            tool_name: "read_file".into(),
            tool_args: serde_json::json!({"path": "/tmp/test"}),
            agent_name: "test".into(),
            session_id: None,
        }).await;

        assert_eq!(decision, HookDecision::Allow);
    }

    #[tokio::test]
    async fn test_metrics_hook_counts() {
        let metrics = MetricsHook::new();

        metrics.post_tool_call(&PostToolCallContext {
            tool_name: "read_file".into(),
            tool_args: serde_json::json!({}),
            result: ToolOutput { success: true, content: "ok".into(), error: None },
            elapsed_ms: 5,
            agent_name: "test".into(),
            session_id: None,
        }).await;
        metrics.post_tool_call(&PostToolCallContext {
            tool_name: "shell".into(),
            tool_args: serde_json::json!({}),
            result: ToolOutput { success: false, content: "".into(), error: Some("fail".into()) },
            elapsed_ms: 10,
            agent_name: "test".into(),
            session_id: None,
        }).await;

        assert_eq!(metrics.tool_call_count().await, 2);
    }

    #[tokio::test]
    async fn test_empty_registry_allows_all() {
        let registry = HookRegistry::new();
        let decision = registry.run_pre_tool_call(&PreToolCallContext {
            tool_name: "any_tool".into(),
            tool_args: serde_json::json!({}),
            agent_name: "test".into(),
            session_id: None,
        }).await;
        assert_eq!(decision, HookDecision::Allow);
    }
}
