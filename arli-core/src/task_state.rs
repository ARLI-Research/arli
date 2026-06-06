//! Shared Harness State — phased task execution trail for ENSO visibility.
//!
//! From "Code as Agent Harness" §4.3: multi-agent systems need a shared code-centric
//! harness substrate — not just a final attestation, but a shared program state that
//! both agents read/write: plans, artifacts, intermediate statuses, check results.
//!
//! TaskState bridges ARLI and ENSO: ARLI writes state at each execution phase,
//! ENSO reads it via ICP or filesystem. The state hash is included in the attestation
//! so ENSO can verify the full execution trail.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

// ============================================================================
// TASK PHASE
// ============================================================================

/// Execution phases for a task — mirrors the PEV loop (§3.4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    /// Contract received from ENSO, agent assigned.
    Created,
    /// Agent is decomposing the task into subtasks.
    Planning,
    /// Agent is executing code, producing artifacts.
    Executing,
    /// Agent is running success checks (tests, linters).
    Verifying,
    /// Attestation built, sent to ENSO.
    Attesting,
    /// ENSO confirmed settlement, payment released.
    Settled,
    /// Execution failed at this phase.
    Failed,
}

impl TaskPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Planning => "planning",
            Self::Executing => "executing",
            Self::Verifying => "verifying",
            Self::Attesting => "attesting",
            Self::Settled => "settled",
            Self::Failed => "failed",
        }
    }
}

// ============================================================================
// TASK ARTIFACT
// ============================================================================

/// A file produced during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifact {
    /// Relative path within the workspace.
    pub path: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// SHA-256 of file contents (if computed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

// ============================================================================
// CHECK RESULT
// ============================================================================

/// Result of running a single success check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    /// The check command/description.
    pub check: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Exit code (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Human-readable output excerpt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

// ============================================================================
// TASK ERROR
// ============================================================================

/// A recorded error during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskError {
    /// Human-readable error message.
    pub message: String,
    /// ISO-8601 timestamp.
    pub at: String,
    /// Attempt number.
    pub attempt: u32,
}

// ============================================================================
// TASK STATE
// ============================================================================

/// Shared harness state for one ENSO contract.
///
/// Persisted to `~/.arli/task_states/<contract_id>.json`.
/// Hash included in attestation so ENSO can verify the trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    /// ENSO contract ID.
    pub contract_id: String,

    /// ARLI agent ID.
    pub agent_id: String,

    /// Human-readable goal.
    pub goal: String,

    /// Current execution phase.
    pub phase: TaskPhase,

    /// Files produced so far.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<TaskArtifact>,

    /// Results of success checks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub check_results: Vec<CheckResult>,

    /// Errors encountered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<TaskError>,

    /// ISO-8601 timestamps for each phase transition.
    #[serde(default)]
    pub phase_timestamps: HashMap<String, String>,

    /// Hash of the TaskContract this execution fulfills (from task_contract.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_contract_hash: Option<String>,

    /// Attestation run_id (set after attestation built).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attestation_run_id: Option<String>,

    /// ENSO transaction ID (set after settlement).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_tx_id: Option<String>,

    /// Total attempts made.
    #[serde(default)]
    pub attempts: u32,
}

impl TaskState {
    /// Create a new task state for a contract.
    pub fn new(contract_id: &str, agent_id: &str, goal: &str) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        let mut phase_timestamps = HashMap::new();
        phase_timestamps.insert(TaskPhase::Created.as_str().to_string(), now.clone());

        Self {
            contract_id: contract_id.to_string(),
            agent_id: agent_id.to_string(),
            goal: goal.to_string(),
            phase: TaskPhase::Created,
            artifacts: vec![],
            check_results: vec![],
            errors: vec![],
            phase_timestamps,
            task_contract_hash: None,
            attestation_run_id: None,
            settlement_tx_id: None,
            attempts: 0,
        }
    }

    /// Transition to a new phase, recording the timestamp.
    pub fn transition_to(&mut self, phase: TaskPhase) {
        self.phase = phase.clone();
        self.phase_timestamps.insert(
            phase.as_str().to_string(),
            chrono::Utc::now().to_rfc3339(),
        );
    }

    /// Record a produced artifact.
    pub fn add_artifact(&mut self, path: &str, size_bytes: u64, sha256: Option<String>) {
        self.artifacts.push(TaskArtifact {
            path: path.to_string(),
            size_bytes,
            sha256,
        });
    }

    /// Record a check result.
    pub fn add_check(&mut self, check: &str, passed: bool, exit_code: Option<i32>, output: Option<String>) {
        self.check_results.push(CheckResult {
            check: check.to_string(),
            passed,
            exit_code,
            output,
        });
    }

    /// Record an error.
    pub fn add_error(&mut self, message: &str) {
        self.errors.push(TaskError {
            message: message.to_string(),
            at: chrono::Utc::now().to_rfc3339(),
            attempt: self.attempts,
        });
    }

    /// Increment attempt counter.
    pub fn increment_attempts(&mut self) {
        self.attempts += 1;
    }

    /// Check if all recorded checks passed.
    pub fn all_checks_passed(&self) -> bool {
        if self.check_results.is_empty() {
            return true; // No checks = nothing to fail
        }
        self.check_results.iter().all(|c| c.passed)
    }

    /// Set the task contract hash and transition from Created to Planning.
    pub fn set_contract(&mut self, contract_hash: &str) {
        self.task_contract_hash = Some(contract_hash.to_string());
        self.transition_to(TaskPhase::Planning);
    }

    /// Mark attestation as built.
    pub fn mark_attested(&mut self, run_id: &str) {
        self.attestation_run_id = Some(run_id.to_string());
        self.transition_to(TaskPhase::Attesting);
    }

    /// Mark as settled with tx_id.
    pub fn mark_settled(&mut self, tx_id: &str) {
        self.settlement_tx_id = Some(tx_id.to_string());
        self.transition_to(TaskPhase::Settled);
    }

    /// Mark as failed with error.
    pub fn mark_failed(&mut self, error: &str) {
        self.add_error(error);
        self.transition_to(TaskPhase::Failed);
    }

    // --- Hashing ---

    /// Compute the canonical SHA-256 hash of this state (without signature fields).
    pub fn hash(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        hex::encode(Sha256::digest(json.as_bytes()))
    }

    // --- Persistence ---

    /// Default path for task state files.
    pub fn default_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".arli").join("task_states")
    }

    /// Path to this contract's state file.
    pub fn state_path(&self) -> PathBuf {
        Self::default_dir().join(format!("{}.json", self.contract_id))
    }

    /// Save state to disk.
    pub fn save(&self) -> Result<(), String> {
        let dir = Self::default_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("create task_states dir: {}", e))?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("serialize task state: {}", e))?;
        let path = self.state_path();
        std::fs::write(&path, &json).map_err(|e| format!("write task state: {}", e))
    }

    /// Load state from disk for a given contract ID.
    pub fn load(contract_id: &str) -> Result<Option<Self>, String> {
        let path = Self::default_dir().join(format!("{}.json", contract_id));
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(&path).map_err(|e| format!("read task state: {}", e))?;
        serde_json::from_str(&data).map(Some).map_err(|e| format!("parse task state: {}", e))
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_task_state() {
        let ts = TaskState::new("contract-1", "agent-abc", "Fix bug #42");
        assert_eq!(ts.contract_id, "contract-1");
        assert_eq!(ts.agent_id, "agent-abc");
        assert_eq!(ts.phase, TaskPhase::Created);
        assert_eq!(ts.artifacts.len(), 0);
        assert_eq!(ts.check_results.len(), 0);
        assert!(ts.phase_timestamps.contains_key("created"));
    }

    #[test]
    fn test_phase_transitions() {
        let mut ts = TaskState::new("c1", "a1", "test");

        ts.transition_to(TaskPhase::Planning);
        assert_eq!(ts.phase, TaskPhase::Planning);

        ts.transition_to(TaskPhase::Executing);
        assert_eq!(ts.phase, TaskPhase::Executing);

        ts.transition_to(TaskPhase::Failed);
        assert_eq!(ts.phase, TaskPhase::Failed);

        assert_eq!(ts.phase_timestamps.len(), 4); // Created + 3 transitions
    }

    #[test]
    fn test_artifacts_and_checks() {
        let mut ts = TaskState::new("c1", "a1", "test");
        ts.add_artifact("src/main.rs", 1024, None);
        ts.add_artifact("tests/test.rs", 512, Some("abc123".into()));
        ts.add_check("cargo test", true, Some(0), None);
        ts.add_check("cargo clippy", false, Some(1), Some("warning: unused".into()));

        assert_eq!(ts.artifacts.len(), 2);
        assert_eq!(ts.check_results.len(), 2);
        assert!(!ts.all_checks_passed());
    }

    #[test]
    fn test_all_checks_passed() {
        let mut ts = TaskState::new("c1", "a1", "test");
        assert!(ts.all_checks_passed()); // Empty — vacuously true

        ts.add_check("lint", true, Some(0), None);
        ts.add_check("test", true, Some(0), None);
        assert!(ts.all_checks_passed());

        ts.add_check("audit", false, Some(1), None);
        assert!(!ts.all_checks_passed());
    }

    #[test]
    fn test_save_and_load() {
        let tmp_contract = "arli-test-contract-xyz";
        let path = TaskState::default_dir().join(format!("{}.json", tmp_contract));
        let _ = std::fs::remove_file(&path);

        let mut ts = TaskState::new(tmp_contract, "agent-test", "Unit test task");
        ts.set_contract("sha256:test-contract-hash");
        ts.add_artifact("output.txt", 100, None);
        ts.mark_attested("run-test-001");
        ts.mark_settled("tx-test-001");
        ts.save().unwrap();

        let loaded = TaskState::load(tmp_contract).unwrap().unwrap();
        assert_eq!(loaded.contract_id, tmp_contract);
        assert_eq!(loaded.phase, TaskPhase::Settled);
        assert_eq!(loaded.artifacts.len(), 1);
        assert_eq!(loaded.attestation_run_id, Some("run-test-001".into()));
        assert_eq!(loaded.settlement_tx_id, Some("tx-test-001".into()));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_nonexistent() {
        let result = TaskState::load("nonexistent-contract-id").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_hash_deterministic() {
        let ts1 = TaskState::new("c1", "a1", "goal");
        // Hash from serialized JSON is deterministic for same data
        let json1 = serde_json::to_string(&ts1).unwrap();
        let ts2: TaskState = serde_json::from_str(&json1).unwrap();
        assert_eq!(ts1.hash(), ts2.hash());
    }

    #[test]
    fn test_hash_changes_with_state() {
        let mut ts1 = TaskState::new("c1", "a1", "goal");
        let ts2 = TaskState::new("c1", "a1", "goal");

        ts1.add_artifact("file.txt", 100, None);
        assert_ne!(ts1.hash(), ts2.hash());
    }
}
