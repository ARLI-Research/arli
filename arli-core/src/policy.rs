//! Approval policy engine — controls which tool calls need human approval.
//!
//! Three possible decisions per tool call:
//! - Allow: execute immediately
//! - Deny: block execution with a reason
//! - NeedsApproval: pause and wait for human confirmation
//!
//! Rules are priority-ordered. The first matching rule determines the decision.
//! Default policy (no matching rule): Allow for read-only, NeedsApproval for writes.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

/// Policy decision for a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// Execute immediately — no approval needed.
    Allow,

    /// Block the tool call entirely.
    Deny {
        reason: String,
    },

    /// Pause execution and wait for human confirmation.
    NeedsApproval {
        reason: String,
    },

    /// Rate limit exceeded — try again later.
    RateLimited {
        reason: String,
        retry_after_secs: u64,
    },
}

impl Decision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Decision::Allow)
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, Decision::Deny { .. } | Decision::RateLimited { .. })
    }

    pub fn needs_approval(&self) -> bool {
        matches!(self, Decision::NeedsApproval { .. })
    }
}

/// Rate limit configuration for a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    /// Maximum calls allowed in the window
    pub max_calls: u32,
    /// Time window in seconds
    pub window_secs: u64,
}

/// A single policy rule — tool name pattern → decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Rule name for logging/debugging
    pub name: String,

    /// Tool name to match. Supports wildcard suffix: "shell*" matches "shell", "shell_exec".
    /// `None` matches ALL tools (catch-all rule).
    pub tool_match: Option<String>,

    /// Optional toolset to match. If set, the tool must belong to this toolset.
    #[serde(default)]
    pub toolset: Option<String>,

    /// The decision for matching tool calls.
    pub decision: Decision,

    /// Priority — higher values checked first. Default rules have priority 0.
    #[serde(default)]
    pub priority: u32,

    /// Optional: only apply to a specific agent profile.
    #[serde(default)]
    pub agent_profile: Option<String>,

    /// Human-readable description of why this rule exists.
    #[serde(default)]
    pub description: String,
}

/// Per-agent trading limits (enforced by policy engine).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TradingLimits {
    /// Maximum position size in USD
    #[serde(default)]
    pub max_position_size_usd: Option<f64>,

    /// Maximum number of trades per day
    #[serde(default)]
    pub max_daily_trades: Option<u32>,

    /// Maximum drawdown percentage before blocking (e.g., 20.0 = 20%)
    #[serde(default)]
    pub max_drawdown_pct: Option<f64>,

    /// Maximum leverage
    #[serde(default)]
    pub max_leverage: Option<u32>,

    /// Allowed trading pairs (empty = all allowed)
    #[serde(default)]
    pub allowed_pairs: Vec<String>,

    /// Blocked trading pairs
    #[serde(default)]
    pub blocked_pairs: Vec<String>,
}

/// The policy engine — evaluates tool calls against rules.
#[derive(Debug, Clone)]
pub struct PolicyEngine {
    rules: Vec<PolicyRule>,
    agent_limits: HashMap<String, TradingLimits>,

    /// Track daily trade count per agent
    daily_trade_counts: HashMap<String, u32>,

    /// Default decision when no rule matches
    default_decision: Decision,

    /// Read-only tool names (always Allow unless explicitly overridden)
    read_only_tools: Vec<String>,

    /// Destructive tool names (default to NeedsApproval)
    destructive_tools: Vec<String>,

    /// Rate limits per tool name pattern
    rate_limits: HashMap<String, RateLimit>,

    /// Call history: tool_name → list of call timestamps
    #[allow(dead_code)]
    call_history: HashMap<String, Vec<Instant>>,
}

impl PolicyEngine {
    /// Create a new policy engine with sensible defaults.
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            agent_limits: HashMap::new(),
            daily_trade_counts: HashMap::new(),
            default_decision: Decision::NeedsApproval {
                reason: "No policy rule matched. Default: requires approval.".into(),
            },
            read_only_tools: vec![
                "read_file".into(),
                "search_files".into(),
                "session_search".into(),
            ],
            destructive_tools: vec![
                "shell".into(),
                "write_file".into(),
                "execute_trade".into(),
                "cancel_order".into(),
            ],
            rate_limits: HashMap::new(),
            call_history: HashMap::new(),
        }
    }

    /// Create a policy engine with default rules installed.
    pub fn with_defaults() -> Self {
        let mut engine = Self::new();

        // Read-only tools → always allow
        engine.add_rule(PolicyRule {
            name: "allow-read-only".into(),
            tool_match: Some("read_file".into()),
            toolset: None,
            decision: Decision::Allow,
            priority: 100,
            agent_profile: None,
            description: "Reading files is always safe".into(),
        });

        engine.add_rule(PolicyRule {
            name: "allow-search".into(),
            tool_match: Some("search_files".into()),
            toolset: None,
            decision: Decision::Allow,
            priority: 100,
            agent_profile: None,
            description: "Searching files is always safe".into(),
        });

        engine.add_rule(PolicyRule {
            name: "allow-session-search".into(),
            tool_match: Some("session_search".into()),
            toolset: None,
            decision: Decision::Allow,
            priority: 100,
            agent_profile: None,
            description: "Session search is always safe".into(),
        });

        engine.add_rule(PolicyRule {
            name: "allow-http-get".into(),
            tool_match: Some("http_get".into()),
            toolset: None,
            decision: Decision::Allow,
            priority: 100,
            agent_profile: None,
            description: "HTTP GET is read-only".into(),
        });

        // Write tools → need approval
        engine.add_rule(PolicyRule {
            name: "approve-write".into(),
            tool_match: Some("write_file".into()),
            toolset: None,
            decision: Decision::NeedsApproval {
                reason: "Writing to filesystem requires confirmation".into(),
            },
            priority: 50,
            agent_profile: None,
            description: "File writes should be reviewed".into(),
        });

        // Shell → need approval
        engine.add_rule(PolicyRule {
            name: "approve-shell".into(),
            tool_match: Some("shell".into()),
            toolset: None,
            decision: Decision::NeedsApproval {
                reason: "Shell commands require confirmation".into(),
            },
            priority: 50,
            agent_profile: None,
            description: "Shell execution should be reviewed".into(),
        });

        // Trading tools → need approval by default
        engine.add_rule(PolicyRule {
            name: "approve-trading".into(),
            tool_match: Some("execute_trade".into()),
            toolset: None,
            decision: Decision::NeedsApproval {
                reason: "Trade execution requires confirmation".into(),
            },
            priority: 50,
            agent_profile: None,
            description: "Trades must be confirmed".into(),
        });

        engine
    }

    /// Add a policy rule. Higher priority rules checked first.
    pub fn add_rule(&mut self, rule: PolicyRule) {
        self.rules.push(rule);
        // Sort by priority descending
        self.rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
    }

    /// Set trading limits for a specific agent profile.
    pub fn set_agent_limits(&mut self, profile: &str, limits: TradingLimits) {
        self.agent_limits.insert(profile.to_string(), limits);
    }

    /// Evaluate a tool call against all rules.
    ///
    /// Returns the decision from the first matching rule, or the default decision.
    pub fn evaluate(
        &self,
        tool_name: &str,
        _tool_args: &serde_json::Value,
        agent_profile: Option<&str>,
    ) -> Decision {
        // Check each rule in priority order
        for rule in &self.rules {
            // Agent profile filter
            if let Some(ref profile) = rule.agent_profile {
                if agent_profile != Some(profile.as_str()) {
                    continue;
                }
            }

            // Tool name match
            let name_matches = match &rule.tool_match {
                None => true, // catch-all
                Some(pattern) => {
                    if pattern.ends_with('*') {
                        let prefix = &pattern[..pattern.len() - 1];
                        tool_name.starts_with(prefix)
                    } else {
                        tool_name == pattern.as_str()
                    }
                }
            };

            if name_matches {
                tracing::debug!(
                    "Policy rule '{}' matched tool '{}' → {:?}",
                    rule.name,
                    tool_name,
                    rule.decision
                );
                return rule.decision.clone();
            }
        }

        // No rule matched — use smart default
        let decision = if self.read_only_tools.contains(&tool_name.to_string()) {
            Decision::Allow
        } else if self.destructive_tools.contains(&tool_name.to_string()) {
            Decision::NeedsApproval {
                reason: format!("Tool '{}' can modify state. Approval required.", tool_name),
            }
        } else {
            self.default_decision.clone()
        };

        tracing::debug!(
            "No policy rule matched '{}' → default {:?}",
            tool_name,
            decision
        );

        decision
    }

    /// Check trading limits for an agent.
    pub fn check_trading_limits(
        &self,
        agent_profile: &str,
        position_size_usd: f64,
        pair: &str,
    ) -> Result<(), String> {
        if let Some(limits) = self.agent_limits.get(agent_profile) {
            // Check position size
            if let Some(max_size) = limits.max_position_size_usd {
                if position_size_usd > max_size {
                    return Err(format!(
                        "Position size ${:.2} exceeds limit ${:.2}",
                        position_size_usd, max_size
                    ));
                }
            }

            // Check blocked pairs
            if limits.blocked_pairs.contains(&pair.to_string()) {
                return Err(format!("Trading pair '{}' is blocked", pair));
            }

            // Check allowed pairs (empty = all allowed)
            if !limits.allowed_pairs.is_empty()
                && !limits.allowed_pairs.contains(&pair.to_string())
            {
                return Err(format!(
                    "Trading pair '{}' not in allowed list: {:?}",
                    pair, limits.allowed_pairs
                ));
            }

            // Check daily trade count
            if let Some(max_trades) = limits.max_daily_trades {
                let count = self.daily_trade_counts.get(agent_profile).copied().unwrap_or(0);
                if count >= max_trades {
                    return Err(format!(
                        "Daily trade limit reached: {}/{}",
                        count, max_trades
                    ));
                }
            }
        }

        Ok(())
    }

    /// Record a trade execution (for daily count tracking).
    pub fn record_trade(&mut self, agent_profile: &str) {
        *self.daily_trade_counts
            .entry(agent_profile.to_string())
            .or_insert(0) += 1;
    }

    /// Set a rate limit for a tool (exact name or pattern like "shell*").
    pub fn set_rate_limit(&mut self, tool_pattern: &str, limit: RateLimit) {
        self.rate_limits.insert(tool_pattern.to_string(), limit);
    }

    /// Check if a tool call exceeds rate limits. Records the call if allowed.
    pub fn check_rate_limit(&mut self, tool_name: &str) -> Decision {
        // Find matching rate limit
        let limit = self.rate_limits.iter().find(|(pattern, _)| {
            if pattern.ends_with('*') {
                tool_name.starts_with(&pattern[..pattern.len()-1])
            } else {
                pattern.as_str() == tool_name
            }
        });

        if let Some((_pattern, limit)) = limit {
            let now = Instant::now();
            let window = std::time::Duration::from_secs(limit.window_secs);
            let cutoff = now.checked_sub(window).unwrap_or(now);

            let history = self.call_history.entry(tool_name.to_string()).or_default();

            // Remove old entries
            history.retain(|t| *t > cutoff);

            if history.len() >= limit.max_calls as usize {
                let oldest = history.first().copied().unwrap_or(now);
                let retry_after = limit.window_secs.saturating_sub((now - oldest).as_secs());
                return Decision::RateLimited {
                    reason: format!(
                        "Rate limit: {} calls per {}s. {} calls made.",
                        limit.max_calls, limit.window_secs, history.len()
                    ),
                    retry_after_secs: retry_after.max(1),
                };
            }

            // Record this call
            history.push(now);
        }

        Decision::Allow
    }

    /// Load rules from a TOML configuration.
    pub fn load_toml(&mut self, toml_str: &str) -> Result<usize, toml::de::Error> {
        #[derive(Deserialize)]
        struct PolicyConfig {
            rules: Vec<PolicyRule>,
            #[serde(default)]
            agent_limits: HashMap<String, TradingLimits>,
        }

        let config: PolicyConfig = toml::from_str(toml_str)?;
        let count = config.rules.len();

        for rule in config.rules {
            self.add_rule(rule);
        }

        for (profile, limits) in config.agent_limits {
            self.set_agent_limits(&profile, limits);
        }

        Ok(count)
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_only_allowed() {
        let engine = PolicyEngine::with_defaults();
        let decision = engine.evaluate("read_file", &serde_json::json!({"path": "/tmp/test"}), None);
        assert!(decision.is_allowed());
    }

    #[test]
    fn test_write_needs_approval() {
        let engine = PolicyEngine::with_defaults();
        let decision = engine.evaluate("write_file", &serde_json::json!({"path": "/tmp/test"}), None);
        assert!(decision.needs_approval());
    }

    #[test]
    fn test_shell_needs_approval() {
        let engine = PolicyEngine::with_defaults();
        let decision = engine.evaluate("shell", &serde_json::json!({"command": "ls"}), None);
        assert!(decision.needs_approval());
    }

    #[test]
    fn test_custom_rule_override() {
        let mut engine = PolicyEngine::new();

        // Add a rule that allows shell for a specific agent
        engine.add_rule(PolicyRule {
            name: "allow-shell-for-backtest".into(),
            tool_match: Some("shell".into()),
            toolset: None,
            decision: Decision::Allow,
            priority: 200,
            agent_profile: Some("backtest".into()),
            description: "Backtest agents can run shell freely".into(),
        });

        // Without the profile → default (needs approval)
        let d1 = engine.evaluate("shell", &serde_json::json!({"cmd": "ls"}), None);
        assert!(d1.needs_approval());

        // With matching profile → allowed
        let d2 = engine.evaluate("shell", &serde_json::json!({"cmd": "ls"}), Some("backtest"));
        assert!(d2.is_allowed());

        // Different profile → still needs approval
        let d3 = engine.evaluate("shell", &serde_json::json!({"cmd": "ls"}), Some("live"));
        assert!(d3.needs_approval());
    }

    #[test]
    fn test_wildcard_matching() {
        let mut engine = PolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "block-all-trades".into(),
            tool_match: Some("execute_*".into()),
            toolset: None,
            decision: Decision::Deny {
                reason: "All trades blocked".into(),
            },
            priority: 999,
            agent_profile: None,
            description: "Block all trade execution".into(),
        });

        assert!(engine.evaluate("execute_trade", &serde_json::json!({}), None).is_denied());
        assert!(engine.evaluate("execute_limit_order", &serde_json::json!({}), None).is_denied());
        // Unrelated tool unaffected
        assert!(engine.evaluate("read_file", &serde_json::json!({}), None).is_allowed());
    }

    #[test]
    fn test_trading_limits() {
        let mut engine = PolicyEngine::new();
        engine.set_agent_limits(
            "live",
            TradingLimits {
                max_position_size_usd: Some(1000.0),
                max_daily_trades: Some(5),
                blocked_pairs: vec!["BTC-USDT".into()],
                ..Default::default()
            },
        );

        // Within limits
        assert!(engine.check_trading_limits("live", 500.0, "ETH-USDT").is_ok());

        // Exceeds position size
        assert!(engine.check_trading_limits("live", 1500.0, "ETH-USDT").is_err());

        // Blocked pair
        assert!(engine.check_trading_limits("live", 100.0, "BTC-USDT").is_err());

        // Daily trade limit
        for _ in 0..5 {
            engine.record_trade("live");
        }
        assert!(engine.check_trading_limits("live", 100.0, "ETH-USDT").is_err());
    }

    #[test]
    fn test_priority_ordering() {
        let mut engine = PolicyEngine::new();

        // Low-priority allow
        engine.add_rule(PolicyRule {
            name: "default-allow-all".into(),
            tool_match: None, // catch-all
            toolset: None,
            decision: Decision::Allow,
            priority: 0,
            agent_profile: None,
            description: "Default allow".into(),
        });

        // High-priority deny for specific tool
        engine.add_rule(PolicyRule {
            name: "deny-dangerous".into(),
            tool_match: Some("shell".into()),
            toolset: None,
            decision: Decision::Deny {
                reason: "Shell blocked".into(),
            },
            priority: 100,
            agent_profile: None,
            description: "Block shell".into(),
        });

        // Higher priority wins
        assert!(engine.evaluate("shell", &serde_json::json!({}), None).is_denied());
        // Catch-all applies when no specific rule
        assert!(engine.evaluate("unknown_tool", &serde_json::json!({}), None).is_allowed());
    }
}
