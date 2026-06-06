//! Task Contracts — upfront declaration of work scope for ENSO attestation.
//!
//! From "Code as Agent Harness" §3.4.2: before executing a complex task,
//! ARLI declares a *contract*: what will be done, expected output artifacts,
//! and success verification checks. After execution, the attestation includes
//! a hash of the contract — turning "agent was sandboxed" into
//! "agent was sandboxed AND delivered exactly what it promised."

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::sandbox_profile::{resolve_profile, SandboxProfile};

// ============================================================================
// AUTO-DISCOVERED CHECKS
// ============================================================================

/// Known test runner configurations for auto-discovery.
const KNOWN_TEST_COMMANDS: &[(&str, &[&str])] = &[
    // (marker file, [test commands])
    ("Cargo.toml", &["cargo test", "cargo clippy -- -D warnings"]),
    ("package.json", &["npm test"]),
    ("pnpm-lock.yaml", &["pnpm test"]),
    ("yarn.lock", &["yarn test"]),
    ("Makefile", &["make test"]),
    ("pyproject.toml", &["pytest", "ruff check ."]),
    ("setup.cfg", &["pytest"]),
    ("tox.ini", &["tox"]),
    ("go.mod", &["go test ./...", "go vet ./..."]),
    (
        "CMakeLists.txt",
        &["cmake --build build && ctest --test-dir build"],
    ),
];

// ============================================================================
// TASK CONTRACT
// ============================================================================

/// A Task Contract declares upfront what an agent execution should produce.
///
/// Hashed and included in the `ArliAttestation` so ENSO can verify the agent
/// delivered on its promise — not just that it was sandboxed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskContract {
    /// Human-readable goal description.
    pub goal: String,

    /// File paths that must exist after execution (relative to workspace root).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_artifacts: Vec<String>,

    /// Shell commands that must exit 0 when run after execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_checks: Vec<String>,

    /// Which sandbox policy this contract expects (e.g. "enso-v1").
    /// Must match the sandbox_config_hash in the attestation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<String>,
}

impl TaskContract {
    /// Compute the canonical SHA-256 hash of this contract.
    ///
    /// Uses sorted JSON keys (serde default) to ensure determinism.
    pub fn hash(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_else(|_| String::new());
        hex::encode(Sha256::digest(json.as_bytes()))
    }

    /// Validate that all expected artifacts exist under `workspace_root`.
    ///
    /// Returns a list of missing files (empty = all present).
    pub fn validate_artifacts(&self, workspace_root: &PathBuf) -> Vec<String> {
        self.expected_artifacts
            .iter()
            .filter(|path| !workspace_root.join(path).exists())
            .cloned()
            .collect()
    }

    /// Returns true if all validation checks pass.
    pub fn is_satisfied(&self, workspace_root: &PathBuf) -> bool {
        self.validate_artifacts(workspace_root).is_empty()
    }

    /// Auto-discover success checks from the workspace.
    ///
    /// Scans for known test runner config files (Cargo.toml, package.json, etc.)
    /// and generates appropriate `success_checks` entries.
    /// Appends to existing checks — never removes manual ones.
    ///
    /// From "Learning to generate unit tests for automated debugging" (COLM 2025):
    /// if the user doesn't specify checks, the harness should discover them.
    pub fn discover_checks(&mut self, workspace_root: &PathBuf) -> usize {
        let before = self.success_checks.len();

        for (marker, commands) in KNOWN_TEST_COMMANDS {
            if workspace_root.join(marker).exists() {
                for cmd in *commands {
                    let cmd_str = cmd.to_string();
                    if !self.success_checks.contains(&cmd_str) {
                        self.success_checks.push(cmd_str);
                    }
                }
            }
        }

        self.success_checks.len() - before
    }

    /// Resolve the sandbox policy to a hierarchical profile.
    ///
    /// Uses `crate::sandbox_profile::resolve_profile()` which handles
    /// simple names ("build"), prefixed forms ("enso-build-v1"), and
    /// defaults to `Build` for None or unknown policies.
    pub fn resolve_sandbox_profile(&self) -> SandboxProfile {
        resolve_profile(self.sandbox_policy.as_deref())
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contract_hash_deterministic() {
        let c1 = TaskContract {
            goal: "fix bug #42".into(),
            expected_artifacts: vec!["src/fix.patch".into(), "tests/test_fix.py".into()],
            success_checks: vec!["cargo test".into()],
            sandbox_policy: Some("enso-v1".into()),
        };

        let c2 = c1.clone();
        assert_eq!(c1.hash(), c2.hash());
    }

    #[test]
    fn test_contract_hash_changes_with_goal() {
        let c1 = TaskContract {
            goal: "fix bug #42".into(),
            expected_artifacts: vec![],
            success_checks: vec![],
            sandbox_policy: None,
        };
        let mut c2 = c1.clone();
        c2.goal = "different goal".into();
        assert_ne!(c1.hash(), c2.hash());
    }

    #[test]
    fn test_contract_hash_changes_with_artifacts() {
        let c1 = TaskContract {
            goal: "fix".into(),
            expected_artifacts: vec!["a.txt".into()],
            success_checks: vec![],
            sandbox_policy: None,
        };
        let mut c2 = c1.clone();
        c2.expected_artifacts = vec!["b.txt".into()];
        assert_ne!(c1.hash(), c2.hash());
    }

    #[test]
    fn test_validate_artifacts_empty_contract() {
        let c = TaskContract {
            goal: "do nothing".into(),
            expected_artifacts: vec![],
            success_checks: vec![],
            sandbox_policy: None,
        };
        assert!(c.is_satisfied(&std::env::temp_dir()));
    }

    #[test]
    fn test_validate_artifacts_missing() {
        let c = TaskContract {
            goal: "something".into(),
            expected_artifacts: vec!["nonexistent_file_xyzzy.txt".into()],
            success_checks: vec![],
            sandbox_policy: None,
        };
        let missing = c.validate_artifacts(&std::env::temp_dir());
        assert_eq!(missing.len(), 1);
        assert!(!c.is_satisfied(&std::env::temp_dir()));
    }

    #[test]
    fn test_validate_artifacts_existing() {
        let tmp = std::env::temp_dir();
        let test_file = tmp.join("arli_task_contract_test.txt");
        std::fs::write(&test_file, "test").unwrap();

        let c = TaskContract {
            goal: "check file exists".into(),
            expected_artifacts: vec!["arli_task_contract_test.txt".into()],
            success_checks: vec![],
            sandbox_policy: None,
        };
        assert!(c.is_satisfied(&tmp));

        let _ = std::fs::remove_file(&test_file);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let c = TaskContract {
            goal: "implement feature X".into(),
            expected_artifacts: vec!["src/feature.rs".into(), "tests/test_feature.rs".into()],
            success_checks: vec![
                "cargo test --lib".into(),
                "cargo clippy -- -D warnings".into(),
            ],
            sandbox_policy: Some("enso-v2".into()),
        };

        let json = serde_json::to_string(&c).unwrap();
        let c2: TaskContract = serde_json::from_str(&json).unwrap();
        assert_eq!(c, c2);
        assert_eq!(c.hash(), c2.hash());
    }

    // --- Auto-discovery tests ---

    #[test]
    fn test_discover_checks_rust_project() {
        let tmp = std::env::temp_dir().join("arli_test_rust_workspace");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        let mut c = TaskContract {
            goal: "fix rust bug".into(),
            expected_artifacts: vec![],
            success_checks: vec![],
            sandbox_policy: None,
        };

        let added = c.discover_checks(&tmp);
        assert!(added >= 2);
        assert!(c.success_checks.contains(&"cargo test".to_string()));
        assert!(c
            .success_checks
            .contains(&"cargo clippy -- -D warnings".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_discover_checks_no_duplicates() {
        let tmp = std::env::temp_dir().join("arli_test_dup_workspace");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]").unwrap();

        let mut c = TaskContract {
            goal: "fix".into(),
            expected_artifacts: vec![],
            success_checks: vec!["cargo test".to_string()], // already present
            sandbox_policy: None,
        };

        let added = c.discover_checks(&tmp);
        assert!(added >= 1); // clippy added, but not duplicate cargo test
        assert_eq!(
            c.success_checks
                .iter()
                .filter(|s| *s == "cargo test")
                .count(),
            1
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_discover_checks_empty_workspace() {
        let tmp = std::env::temp_dir().join("arli_test_empty_workspace");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut c = TaskContract {
            goal: "fix".into(),
            expected_artifacts: vec![],
            success_checks: vec![],
            sandbox_policy: None,
        };

        let added = c.discover_checks(&tmp);
        assert_eq!(added, 0);
        assert!(c.success_checks.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
