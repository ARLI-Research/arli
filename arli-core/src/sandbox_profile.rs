//! Hierarchical Sandbox Profiles — predefined security levels for agent execution.
//!
//! From "Towards Secure and Reliable AI Agent Harnesses" (2025):
//! sandbox profiles should be hierarchical — each level adds restrictions
//! on top of the previous, so moving "up" (more permissive) is an explicit
//! escalation, not an accident.
//!
//! Four profiles, from most restrictive to least:
//!   Build → Test → Deploy → Unsafe
//!
//! Each TaskContract declares which profile it needs. The harness enforces
//! that the actual sandbox config is AT LEAST as restrictive as the contract
//! demands.

use crate::sandbox::SandboxConfig;

// ============================================================================
// SANDBOX PROFILE
// ============================================================================

/// Hierarchical sandbox profile — determines isolation level for contract execution.
///
/// Profiles form a strict hierarchy: Build ⊂ Test ⊂ Deploy ⊂ Unsafe.
/// A contract requesting `Test` can run under `Test`, `Deploy`, or `Unsafe`
/// (less restrictive), but NOT under `Build` (more restrictive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SandboxProfile {
    /// Most restrictive — compile/lint only.
    /// No network, read-only filesystem (except target/), 1GB RAM, 5min timeout.
    Build = 0,

    /// Build + test execution.
    /// Limited network (test fixtures), read-only source + write test outputs, 2GB, 10min.
    Test = 1,

    /// Full workspace access, network allowed.
    /// Deploy scripts, integration tests, staging. 4GB, 20min.
    Deploy = 2,

    /// No restrictions — full system access.
    /// Only for trusted, reviewed code. No resource limits.
    Unsafe = 3,
}

impl SandboxProfile {
    /// Parse from string (case-insensitive).
    ///
    /// Accepts: "build", "test", "deploy", "unsafe".
    /// Returns None for unknown strings.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "build" => Some(Self::Build),
            "test" => Some(Self::Test),
            "deploy" => Some(Self::Deploy),
            "unsafe" => Some(Self::Unsafe),
            _ => None,
        }
    }

    /// Returns the least restrictive profile (Unsafe).
    pub fn most_permissive() -> Self {
        Self::Unsafe
    }

    /// Returns the most restrictive profile (Build).
    pub fn most_restrictive() -> Self {
        Self::Build
    }

    /// Check if `self` is at least as permissive as `required`.
    ///
    /// ```
    /// use arli_core::sandbox_profile::SandboxProfile;
    ///
    /// assert!(SandboxProfile::Test.satisfies(SandboxProfile::Build));
    /// assert!(SandboxProfile::Deploy.satisfies(SandboxProfile::Test));
    /// assert!(!SandboxProfile::Build.satisfies(SandboxProfile::Deploy));
    /// ```
    pub fn satisfies(self, required: SandboxProfile) -> bool {
        self >= required
    }

    /// Convert profile to the corresponding `SandboxConfig`.
    ///
    /// Each profile maps to specific namespace, resource, and network settings.
    pub fn to_sandbox_config(self) -> SandboxConfig {
        match self {
            Self::Build => SandboxConfig {
                mount_ns: true,
                net_ns: true,
                pid_ns: true,
                uts_ns: true,
                ipc_ns: true,
                allow_network: false,
                memory_limit_bytes: 1_073_741_824, // 1 GB
                cpu_time_limit_secs: 0,
                max_file_size_bytes: 500 * 1024 * 1024, // 500 MB
                timeout_secs: 300,                      // 5 min
                ..SandboxConfig::default()
            },
            Self::Test => SandboxConfig {
                mount_ns: true,
                net_ns: true,
                pid_ns: true,
                uts_ns: true,
                ipc_ns: true,
                allow_network: true, // test fixtures may need network
                memory_limit_bytes: 2_147_483_648, // 2 GB
                cpu_time_limit_secs: 0,
                max_file_size_bytes: 1_000 * 1024 * 1024, // 1 GB
                timeout_secs: 600,                        // 10 min
                ..SandboxConfig::default()
            },
            Self::Deploy => SandboxConfig {
                mount_ns: true,
                net_ns: false, // full network access (no net namespace)
                pid_ns: true,
                uts_ns: false,
                ipc_ns: false,
                allow_network: true,
                memory_limit_bytes: 4_294_967_296, // 4 GB
                cpu_time_limit_secs: 0,
                max_file_size_bytes: 0, // no limit
                timeout_secs: 1200,     // 20 min
                ..SandboxConfig::default()
            },
            Self::Unsafe => SandboxConfig::permissive(),
        }
    }

    /// Human-readable description of what this profile allows.
    pub fn description(self) -> &'static str {
        match self {
            Self::Build => "Compile/lint only — no network, read-only filesystem, 1GB RAM, 5min",
            Self::Test => "Build + test — limited network, 2GB RAM, 10min",
            Self::Deploy => "Full workspace — network allowed, 4GB RAM, 20min",
            Self::Unsafe => "No restrictions — full system access, no limits",
        }
    }

    /// Returns the default profile for unknown/unspecified contracts.
    pub fn default_for_unknown() -> Self {
        Self::Build // safest default
    }
}

impl std::fmt::Display for SandboxProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build => write!(f, "build"),
            Self::Test => write!(f, "test"),
            Self::Deploy => write!(f, "deploy"),
            Self::Unsafe => write!(f, "unsafe"),
        }
    }
}

// ============================================================================
// PROFILE RESOLVER
// ============================================================================

/// Resolve a sandbox policy string (from TaskContract.sandbox_policy) to a profile.
///
/// Policy strings can be:
/// - Simple profile name: "build", "test", "deploy", "unsafe"
/// - Prefixed: "enso-build-v1", "arli:deploy"
/// - Unknown/None → returns Build (safest default)
pub fn resolve_profile(policy: Option<&str>) -> SandboxProfile {
    match policy {
        None => SandboxProfile::default_for_unknown(),
        Some(p) => {
            // Try exact match first
            if let Some(profile) = SandboxProfile::from_str(p) {
                return profile;
            }

            // Try prefixed forms: "enso-<profile>-v1", "arli:<profile>"
            let lower = p.to_lowercase();

            // "enso-build-v1" → extract "build"
            for prefix in &["enso-", "arli:", "arli-"] {
                if let Some(rest) = lower.strip_prefix(prefix) {
                    // Strip version suffixes: "-v1", "-v2", etc.
                    let base = rest.split('-').next().unwrap_or(rest);
                    if let Some(profile) = SandboxProfile::from_str(base) {
                        return profile;
                    }
                }
            }

            // Unknown policy → safest default
            tracing::warn!("Unknown sandbox policy '{}', defaulting to Build", p);
            SandboxProfile::default_for_unknown()
        }
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Profile parsing ---

    #[test]
    fn test_parse_all_profiles() {
        assert_eq!(
            SandboxProfile::from_str("build"),
            Some(SandboxProfile::Build)
        );
        assert_eq!(
            SandboxProfile::from_str("BUILD"),
            Some(SandboxProfile::Build)
        );
        assert_eq!(SandboxProfile::from_str("Test"), Some(SandboxProfile::Test));
        assert_eq!(
            SandboxProfile::from_str("Deploy"),
            Some(SandboxProfile::Deploy)
        );
        assert_eq!(
            SandboxProfile::from_str("unsafe"),
            Some(SandboxProfile::Unsafe)
        );
    }

    #[test]
    fn test_parse_unknown() {
        assert_eq!(SandboxProfile::from_str("nuclear"), None);
        assert_eq!(SandboxProfile::from_str(""), None);
    }

    // --- Hierarchy ---

    #[test]
    fn test_hierarchy_order() {
        assert!(SandboxProfile::Build < SandboxProfile::Test);
        assert!(SandboxProfile::Test < SandboxProfile::Deploy);
        assert!(SandboxProfile::Deploy < SandboxProfile::Unsafe);
    }

    #[test]
    fn test_satisfies() {
        // Same level
        assert!(SandboxProfile::Build.satisfies(SandboxProfile::Build));
        // More permissive satisfies more restrictive
        assert!(SandboxProfile::Test.satisfies(SandboxProfile::Build));
        assert!(SandboxProfile::Deploy.satisfies(SandboxProfile::Test));
        assert!(SandboxProfile::Unsafe.satisfies(SandboxProfile::Build));
        // Restrictive does NOT satisfy permissive
        assert!(!SandboxProfile::Build.satisfies(SandboxProfile::Test));
        assert!(!SandboxProfile::Test.satisfies(SandboxProfile::Deploy));
        assert!(!SandboxProfile::Deploy.satisfies(SandboxProfile::Unsafe));
    }

    // --- Config generation ---

    #[test]
    fn test_build_config_no_network() {
        let config = SandboxProfile::Build.to_sandbox_config();
        assert!(!config.allow_network);
        assert!(config.net_ns);
        assert_eq!(config.memory_limit_bytes, 1_073_741_824);
        assert_eq!(config.timeout_secs, 300);
    }

    #[test]
    fn test_test_config_has_network() {
        let config = SandboxProfile::Test.to_sandbox_config();
        assert!(config.allow_network);
        assert_eq!(config.memory_limit_bytes, 2_147_483_648);
        assert_eq!(config.timeout_secs, 600);
    }

    #[test]
    fn test_deploy_config_full_network() {
        let config = SandboxProfile::Deploy.to_sandbox_config();
        assert!(config.allow_network);
        assert!(!config.net_ns); // no net namespace = full access
        assert_eq!(config.memory_limit_bytes, 4_294_967_296);
        assert_eq!(config.timeout_secs, 1200);
    }

    #[test]
    fn test_unsafe_config_permissive() {
        let config = SandboxProfile::Unsafe.to_sandbox_config();
        assert!(config.allow_network);
        assert_eq!(config.memory_limit_bytes, 0); // no limit
        assert_eq!(config.timeout_secs, 300); // permissive default
    }

    // --- Profile resolution ---

    #[test]
    fn test_resolve_simple() {
        assert_eq!(resolve_profile(Some("build")), SandboxProfile::Build);
        assert_eq!(resolve_profile(Some("test")), SandboxProfile::Test);
        assert_eq!(resolve_profile(Some("deploy")), SandboxProfile::Deploy);
        assert_eq!(resolve_profile(Some("unsafe")), SandboxProfile::Unsafe);
    }

    #[test]
    fn test_resolve_none_defaults_to_build() {
        assert_eq!(resolve_profile(None), SandboxProfile::Build);
    }

    #[test]
    fn test_resolve_enso_prefixed() {
        assert_eq!(
            resolve_profile(Some("enso-build-v1")),
            SandboxProfile::Build
        );
        assert_eq!(resolve_profile(Some("enso-test-v2")), SandboxProfile::Test);
        assert_eq!(
            resolve_profile(Some("enso-deploy-v1")),
            SandboxProfile::Deploy
        );
    }

    #[test]
    fn test_resolve_arli_prefixed() {
        assert_eq!(resolve_profile(Some("arli:build")), SandboxProfile::Build);
        assert_eq!(resolve_profile(Some("arli:test")), SandboxProfile::Test);
        assert_eq!(resolve_profile(Some("arli-deploy")), SandboxProfile::Deploy);
    }

    #[test]
    fn test_resolve_unknown_defaults_to_build() {
        assert_eq!(
            resolve_profile(Some("super-duper-secure")),
            SandboxProfile::Build
        );
    }

    // --- Display ---

    #[test]
    fn test_display() {
        assert_eq!(SandboxProfile::Build.to_string(), "build");
        assert_eq!(SandboxProfile::Test.to_string(), "test");
        assert_eq!(SandboxProfile::Deploy.to_string(), "deploy");
        assert_eq!(SandboxProfile::Unsafe.to_string(), "unsafe");
    }

    // --- Description ---

    #[test]
    fn test_description_not_empty() {
        for profile in &[
            SandboxProfile::Build,
            SandboxProfile::Test,
            SandboxProfile::Deploy,
            SandboxProfile::Unsafe,
        ] {
            assert!(!profile.description().is_empty());
        }
    }
}
