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
}
