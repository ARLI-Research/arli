//! Policy engine — evaluates allow/deny decisions for network endpoints.
//!
//! Host glob matching (OpenShell pattern):
//! - Exact match: `api.github.com`
//! - Single-label wildcard: `*.example.com` → matches `sub.example.com`
//! - Recursive wildcard: `**.example.com` → matches `a.b.example.com`
//!
//! Default-deny posture: if no rule matches, access is denied.

use super::policy::{NetworkMode, SandboxPolicy};

/// Compiled policy engine for fast per-request evaluation.
pub struct PolicyEngine {
    endpoints: Vec<CompiledEndpoint>,
}

/// Pre-parsed endpoint with wildcard expansion.
struct CompiledEndpoint {
    /// Original pattern (used for exact match comparison)
    pattern: String,
    wildcard_mode: WildcardMode,
    port: Option<u16>,
}

#[derive(Debug, PartialEq)]
enum WildcardMode {
    /// Exact match required
    Exact,
    /// Single-label wildcard: `*.example.com`
    SingleLabel { suffix: String },
    /// Recursive wildcard: `**.example.com`
    Recursive { suffix: String },
}

impl PolicyEngine {
    /// Create a new engine from a policy.
    pub fn new(policy: &SandboxPolicy) -> Self {
        let endpoints = policy
            .network
            .allowed_endpoints
            .iter()
            .map(|e| CompiledEndpoint {
                wildcard_mode: Self::parse_pattern(&e.host),
                pattern: e.host.clone(),
                port: e.port,
            })
            .collect();

        Self { endpoints }
    }

    /// Check if a host:port pair is allowed by policy.
    ///
    /// Returns `true` if any endpoint matches both host and port.
    /// If network mode is `Allow`, all access is permitted.
    /// If `Block`, all access is denied (regardless of endpoints).
    pub fn is_allowed(&self, mode: &NetworkMode, host: &str, port: u16) -> bool {
        match mode {
            NetworkMode::Allow => true,
            NetworkMode::Block => false,
            NetworkMode::Proxy => {
                for endpoint in &self.endpoints {
                    if Self::host_matches(&endpoint.wildcard_mode, &endpoint.pattern, host) {
                        if let Some(allowed_port) = endpoint.port {
                            if allowed_port == port {
                                return true;
                            }
                        } else {
                            return true; // Any port allowed
                        }
                    }
                }
                false
            }
        }
    }

    /// Parse host pattern into wildcard mode.
    fn parse_pattern(pattern: &str) -> WildcardMode {
        if let Some(rest) = pattern.strip_prefix("**.") {
            WildcardMode::Recursive {
                suffix: rest.to_string(),
            }
        } else if let Some(rest) = pattern.strip_prefix("*.") {
            WildcardMode::SingleLabel {
                suffix: rest.to_string(),
            }
        } else {
            WildcardMode::Exact
        }
    }

    /// Match a host against a wildcard mode.
    fn host_matches(mode: &WildcardMode, pattern: &str, host: &str) -> bool {
        match mode {
            WildcardMode::Exact => host.eq_ignore_ascii_case(pattern),
            WildcardMode::SingleLabel { suffix } => {
                // *.example.com matches sub.example.com but NOT example.com
                host.ends_with(suffix)
                    && host.len() > suffix.len()
                    && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
                    && !host[..host.len() - suffix.len() - 1].contains('.')
            }
            WildcardMode::Recursive { suffix } => {
                // **.example.com matches sub.example.com AND a.b.example.com AND example.com
                host == suffix.as_str()
                    || (host.ends_with(suffix)
                        && host.as_bytes()[host.len() - suffix.len() - 1] == b'.')
            }
        }
    }
}

impl SandboxPolicy {
    /// Load the compiled default policy (embedded YAML).
    pub fn default_policy() -> Self {
        let yaml = include_str!("default_policy.yaml");
        Self::from_yaml(yaml).expect("default_policy.yaml must be valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_allows_github() {
        let policy = SandboxPolicy::default_policy();
        let engine = PolicyEngine::new(&policy);
        assert!(engine.is_allowed(&policy.network.mode, "api.github.com", 443));
    }

    #[test]
    fn test_default_policy_allows_deepseek() {
        let policy = SandboxPolicy::default_policy();
        let engine = PolicyEngine::new(&policy);
        assert!(engine.is_allowed(&policy.network.mode, "api.deepseek.com", 443));
    }

    #[test]
    fn test_default_policy_denies_unknown() {
        let policy = SandboxPolicy::default_policy();
        let engine = PolicyEngine::new(&policy);
        assert!(!engine.is_allowed(&policy.network.mode, "evil.com", 443));
    }

    #[test]
    fn test_block_mode_denies_everything() {
        let mut policy = SandboxPolicy::default_policy();
        policy.network.mode = NetworkMode::Block;
        let engine = PolicyEngine::new(&policy);
        assert!(!engine.is_allowed(&policy.network.mode, "api.github.com", 443));
    }

    #[test]
    fn test_allow_mode_allows_everything() {
        let mut policy = SandboxPolicy::default_policy();
        policy.network.mode = NetworkMode::Allow;
        let engine = PolicyEngine::new(&policy);
        assert!(engine.is_allowed(&policy.network.mode, "evil.com", 666));
    }

    #[test]
    fn test_single_label_wildcard() {
        let mode = WildcardMode::SingleLabel {
            suffix: "example.com".into(),
        };
        let pattern = "*.example.com";
        assert!(PolicyEngine::host_matches(
            &mode,
            pattern,
            "sub.example.com"
        ));
        assert!(!PolicyEngine::host_matches(&mode, pattern, "example.com"));
        assert!(!PolicyEngine::host_matches(
            &mode,
            pattern,
            "a.b.example.com"
        ));
    }

    #[test]
    fn test_recursive_wildcard() {
        let mode = WildcardMode::Recursive {
            suffix: "example.com".into(),
        };
        let pattern = "**.example.com";
        assert!(PolicyEngine::host_matches(&mode, pattern, "example.com"));
        assert!(PolicyEngine::host_matches(
            &mode,
            pattern,
            "sub.example.com"
        ));
        assert!(PolicyEngine::host_matches(
            &mode,
            pattern,
            "a.b.example.com"
        ));
        assert!(!PolicyEngine::host_matches(
            &mode,
            pattern,
            "notexample.com"
        ));
    }

    #[test]
    fn test_exact_match() {
        let mode = WildcardMode::Exact;
        let pattern = "api.github.com";
        assert!(PolicyEngine::host_matches(&mode, pattern, "api.github.com"));
        assert!(!PolicyEngine::host_matches(
            &mode,
            pattern,
            "other.github.com"
        ));
    }

    #[test]
    fn test_wrong_port_denied() {
        let mut policy = SandboxPolicy::default_policy();
        policy.network.mode = NetworkMode::Proxy;
        // deepseek only allows port 443; no wildcard covers it
        let engine = PolicyEngine::new(&policy);
        assert!(!engine.is_allowed(&policy.network.mode, "api.deepseek.com", 80));
        assert!(engine.is_allowed(&policy.network.mode, "api.deepseek.com", 443));
    }

    #[test]
    fn test_default_policy_embedded() {
        let policy = SandboxPolicy::default_policy();
        assert_eq!(policy.network.mode, NetworkMode::Proxy);
        assert!(!policy.filesystem.read_only.is_empty());
        assert!(!policy.filesystem.read_write.is_empty());
        assert!(!policy.network.allowed_endpoints.is_empty());
    }
}
