//! Time-Traveling Stream Rules (TTSR) — response-level rule injection.
//!
//! Rules are regex patterns that, when matched against the model's response,
//! trigger automatic retry with the rule injected as a system reminder. This
//! avoids paying context tax on every turn — rules sit dormant until triggered.
//!
//! ## How it works
//!
//! 1. Model generates a response (full text, non-streaming in ARLI)
//! 2. Response is checked against all active rules
//! 3. If a rule matches → the rule is injected as a system message
//! 4. The current turn is retried with the rule in context
//!
//! Rules survive compaction (injected as system messages), so the fix sticks.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Maximum retries per turn to prevent infinite loops.
const MAX_RETRIES_PER_TURN: usize = 3;

/// A single TTSR rule — regex pattern + injection message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamRule {
    /// Human-readable rule name (e.g. "no-box-leak")
    pub name: String,

    /// Regex pattern to match against model output.
    /// Uses Rust regex syntax (not PCRE — no lookahead/backreferences).
    pub pattern: String,

    /// Message injected when this rule triggers.
    /// Injected as a user message (for maximum attention from the model).
    pub message: String,

    /// Whether this rule is active.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Priority: lower = checked first. Rules stop at first match.
    #[serde(default)]
    pub priority: u32,
}

fn default_true() -> bool {
    true
}

/// Collection of stream rules with matching engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamRules {
    /// All configured rules.
    #[serde(default)]
    pub rules: Vec<StreamRule>,

    /// Whether TTSR is enabled globally.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum retries per turn (default: 3).
    #[serde(default = "default_max_retries")]
    pub max_retries: usize,
}

impl Default for StreamRules {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            enabled: true,
            max_retries: MAX_RETRIES_PER_TURN,
        }
    }
}

fn default_max_retries() -> usize {
    MAX_RETRIES_PER_TURN
}

/// Result of checking a response against rules.
#[derive(Debug)]
pub struct RuleMatch {
    /// The rule that was triggered.
    pub rule: StreamRule,

    /// The matched text snippet (first 100 chars).
    pub matched_snippet: String,
}

impl StreamRules {
    /// Create empty rules set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load rules from a TOML string.
    pub fn from_toml(toml_str: &str) -> Result<Self, String> {
        toml::from_str(toml_str).map_err(|e| format!("invalid stream_rules TOML: {e}"))
    }

    /// Check model response text against all active rules.
    ///
    /// Returns the first matching rule (highest priority, lowest index).
    /// Returns `None` if no rules match.
    pub fn check(&self, response_text: &str) -> Option<RuleMatch> {
        if !self.enabled || self.rules.is_empty() {
            return None;
        }

        // Sort by priority (then index-stable if equal priority)
        let mut indexed: Vec<(usize, &StreamRule)> = self.rules.iter().enumerate().collect();
        indexed.sort_by_key(|(i, r)| (r.priority, *i));

        for (_idx, rule) in indexed {
            if !rule.enabled {
                continue;
            }

            match Regex::new(&rule.pattern) {
                Ok(re) => {
                    if let Some(mat) = re.find(response_text) {
                        let snippet = &response_text[mat.start()..mat.end()];
                        let max_len = 100;
                        let truncated = if snippet.len() > max_len {
                            format!("{}...", &snippet[..max_len])
                        } else {
                            snippet.to_string()
                        };

                        return Some(RuleMatch {
                            rule: rule.clone(),
                            matched_snippet: truncated,
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Stream rule '{}' has invalid regex '{}': {}",
                        rule.name,
                        rule.pattern,
                        e
                    );
                }
            }
        }

        None
    }

    /// Build the injection message for a matched rule.
    ///
    /// Formatted as a user message that grabs the model's attention.
    pub fn build_injection(rule: &StreamRule) -> String {
        format!(
            "⚠ RULE VIOLATION DETECTED: {}\n\n{}\n\n\
             The previous response was blocked. Please adjust your approach \
             to comply with this rule.",
            rule.name, rule.message
        )
    }

    /// Add a rule programmatically.
    pub fn add_rule(&mut self, name: &str, pattern: &str, message: &str) {
        self.rules.push(StreamRule {
            name: name.to_string(),
            pattern: pattern.to_string(),
            message: message.to_string(),
            enabled: true,
            priority: 0,
        });
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_rules_no_match() {
        let rules = StreamRules::new();
        assert!(rules.check("any text").is_none());
    }

    #[test]
    fn test_disabled_rules_no_match() {
        let mut rules = StreamRules::new();
        rules.enabled = false;
        rules.add_rule("test", "forbidden", "Don't use this");
        assert!(rules.check("this is forbidden text").is_none());
    }

    #[test]
    fn test_simple_match() {
        let mut rules = StreamRules::new();
        rules.add_rule("no-passwords", r"password\s*=", "Never hardcode passwords.");
        let m = rules.check("const password = 'secret123';");
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.rule.name, "no-passwords");
        assert!(m.matched_snippet.contains("password"));
    }

    #[test]
    fn test_no_match_on_safe_text() {
        let mut rules = StreamRules::new();
        rules.add_rule(
            "no-box-leak",
            r"Box::leak",
            "Don't use Box::leak in production.",
        );
        assert!(rules
            .check("use std::rc::Rc; let x = Rc::new(42);")
            .is_none());
    }

    #[test]
    fn test_priority_first_match() {
        let mut rules = StreamRules::new();
        rules.rules.push(StreamRule {
            name: "low".into(),
            pattern: "eval".into(),
            message: "low priority".into(),
            enabled: true,
            priority: 10,
        });
        rules.rules.push(StreamRule {
            name: "high".into(),
            pattern: "eval".into(),
            message: "high priority".into(),
            enabled: true,
            priority: 1,
        });

        let m = rules.check("use eval() to run code");
        assert_eq!(m.unwrap().rule.name, "high");
    }

    #[test]
    fn test_invalid_regex_skipped() {
        let mut rules = StreamRules::new();
        rules.rules.push(StreamRule {
            name: "broken".into(),
            pattern: "[invalid".into(),
            message: "skip me".into(),
            enabled: true,
            priority: 0,
        });
        rules.add_rule("good", "test", "good rule");
        let m = rules.check("this is a test");
        assert!(m.is_some());
        assert_eq!(m.unwrap().rule.name, "good");
    }

    #[test]
    fn test_disabled_rule_skipped() {
        let mut rules = StreamRules::new();
        rules.rules.push(StreamRule {
            name: "disabled".into(),
            pattern: "forbidden".into(),
            message: "should not fire".into(),
            enabled: false,
            priority: 0,
        });
        rules.add_rule("active", "allowed", "active rule");
        // "forbidden" should not match because rule is disabled
        assert!(rules.check("this is forbidden").is_none());
        // "allowed" should still match
        assert!(rules.check("this is allowed").is_some());
    }

    #[test]
    fn test_build_injection() {
        let rule = StreamRule {
            name: "no-eval".into(),
            pattern: "eval".into(),
            message: "eval() is blocked by policy.".into(),
            enabled: true,
            priority: 0,
        };
        let injection = StreamRules::build_injection(&rule);
        assert!(injection.contains("RULE VIOLATION"));
        assert!(injection.contains("no-eval"));
        assert!(injection.contains("eval() is blocked"));
    }

    #[test]
    fn test_from_toml() {
        let toml_str = r#"
enabled = true
max_retries = 3

[[rules]]
name = "no-eval"
pattern = 'eval\s*\('
message = "eval() is blocked by security policy."
enabled = true
priority = 0

[[rules]]
name = "no-unsafe"
pattern = "unsafe\\s*\\{"
message = "unsafe blocks are forbidden."
enabled = true
priority = 1
"#;
        let rules = StreamRules::from_toml(toml_str).unwrap();
        assert_eq!(rules.rules.len(), 2);
        assert!(rules.enabled);

        let m = rules.check("fn foo() { eval(code); }");
        assert_eq!(m.unwrap().rule.name, "no-eval");
    }

    #[test]
    fn test_multiple_matches_first_wins() {
        let mut rules = StreamRules::new();
        rules.add_rule("rule1", "eval", "first");
        rules.add_rule("rule2", "eval", "second");
        let m = rules.check("eval('test')");
        assert_eq!(m.unwrap().rule.name, "rule1");
    }
}
