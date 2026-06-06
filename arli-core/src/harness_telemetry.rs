//! Harness Telemetry — per-tool, per-policy, per-task metrics.
//!
//! From "Code as Agent Harness" §3.5.1: track which tools fail most,
//! which sandbox policies are too tight, and which task types retry most.
//! Feeds data back into harness optimization.
//!
//! Counters are per-key (not just global aggregates) so operators can:
//!   - See which tool fails 40% of calls
//!   - Identify which syscall policy blocks common operations
//!   - Know which task types burn budget on retries

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

// ============================================================================
// TELEMETRY REPORT
// ============================================================================

/// Snapshot of all harness telemetry — serializable for API/dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HarnessTelemetryReport {
    /// Per-tool call counts: tool_name → total calls.
    pub tool_calls: HashMap<String, u64>,

    /// Per-tool failure counts: tool_name → failed calls.
    pub tool_failures: HashMap<String, u64>,

    /// Per-sandbox-policy violation counts: policy_hash → violation count.
    pub policy_violations: HashMap<String, u64>,

    /// Per-task-type retry counts: task_type → total retries.
    pub task_retries: HashMap<String, u64>,

    /// Total unique tool types tracked.
    pub unique_tools: usize,

    /// Total unique policies tracked.
    pub unique_policies: usize,

    /// Total unique task types tracked.
    pub unique_task_types: usize,

    /// Total calls across all tools.
    pub total_tool_calls: u64,

    /// Total failures across all tools.
    pub total_tool_failures: u64,

    /// Total retries across all task types.
    pub total_retries: u64,
}

// ============================================================================
// HARNESS TELEMETRY
// ============================================================================

/// Thread-safe harness telemetry collector.
///
/// Use from any thread — all inner maps are protected by a single Mutex.
/// Designed for low-overhead: lock, increment, unlock.
#[derive(Debug, Default)]
pub struct HarnessTelemetry {
    inner: Mutex<HarnessTelemetryInner>,
}

#[derive(Debug, Default)]
struct HarnessTelemetryInner {
    tool_calls: HashMap<String, u64>,
    tool_failures: HashMap<String, u64>,
    policy_violations: HashMap<String, u64>,
    task_retries: HashMap<String, u64>,
}

impl HarnessTelemetry {
    /// Create a new empty telemetry collector.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HarnessTelemetryInner::default()),
        }
    }

    // --- Recording ---

    /// Record a tool call (success or failure).
    pub fn record_tool_call(&self, tool_name: &str) {
        let mut inner = self.inner.lock().unwrap();
        *inner.tool_calls.entry(tool_name.to_string()).or_insert(0) += 1;
    }

    /// Record a tool failure.
    pub fn record_tool_failure(&self, tool_name: &str) {
        let mut inner = self.inner.lock().unwrap();
        *inner.tool_failures.entry(tool_name.to_string()).or_insert(0) += 1;
    }

    /// Record both a call and a failure in one lock acquisition.
    pub fn record_tool_call_result(&self, tool_name: &str, success: bool) {
        let mut inner = self.inner.lock().unwrap();
        *inner.tool_calls.entry(tool_name.to_string()).or_insert(0) += 1;
        if !success {
            *inner.tool_failures.entry(tool_name.to_string()).or_insert(0) += 1;
        }
    }

    /// Record a sandbox policy violation for a given policy hash.
    pub fn record_policy_violation(&self, policy_hash: &str) {
        let mut inner = self.inner.lock().unwrap();
        *inner
            .policy_violations
            .entry(policy_hash.to_string())
            .or_insert(0) += 1;
    }

    /// Record a retry for a task type.
    pub fn record_retry(&self, task_type: &str) {
        let mut inner = self.inner.lock().unwrap();
        *inner
            .task_retries
            .entry(task_type.to_string())
            .or_insert(0) += 1;
    }

    // --- Reporting ---

    /// Generate a snapshot report of all telemetry data.
    pub fn report(&self) -> HarnessTelemetryReport {
        let inner = self.inner.lock().unwrap();

        let total_tool_calls: u64 = inner.tool_calls.values().sum();
        let total_tool_failures: u64 = inner.tool_failures.values().sum();
        let total_retries: u64 = inner.task_retries.values().sum();

        HarnessTelemetryReport {
            tool_calls: inner.tool_calls.clone(),
            tool_failures: inner.tool_failures.clone(),
            policy_violations: inner.policy_violations.clone(),
            task_retries: inner.task_retries.clone(),
            unique_tools: inner.tool_calls.len(),
            unique_policies: inner.policy_violations.len(),
            unique_task_types: inner.task_retries.len(),
            total_tool_calls,
            total_tool_failures,
            total_retries,
        }
    }

    /// Generate a JSON report string.
    pub fn report_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&self.report())
            .map_err(|e| format!("serialize telemetry: {}", e))
    }

    /// Get failure rate for a specific tool (0.0–1.0).
    /// Returns None if the tool has no recorded calls.
    pub fn failure_rate(&self, tool_name: &str) -> Option<f64> {
        let inner = self.inner.lock().unwrap();
        let calls = inner.tool_calls.get(tool_name)?;
        let failures = inner.tool_failures.get(tool_name).unwrap_or(&0);
        if *calls == 0 {
            None
        } else {
            Some(*failures as f64 / *calls as f64)
        }
    }

    /// Find the most failing tool (name, failure count).
    pub fn most_failing_tool(&self) -> Option<(String, u64)> {
        let inner = self.inner.lock().unwrap();
        inner
            .tool_failures
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(name, &count)| (name.clone(), count))
    }

    /// Find the most violated sandbox policy.
    pub fn most_violated_policy(&self) -> Option<(String, u64)> {
        let inner = self.inner.lock().unwrap();
        inner
            .policy_violations
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(hash, &count)| (hash.clone(), count))
    }

    /// Reset all counters. Useful for unit tests.
    pub fn reset(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.tool_calls.clear();
        inner.tool_failures.clear();
        inner.policy_violations.clear();
        inner.task_retries.clear();
    }
}

// ============================================================================
// PERSISTENCE
// ============================================================================

impl HarnessTelemetry {
    /// Default path: `~/.arli/telemetry.json`
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".arli").join("telemetry.json")
    }

    /// Save current telemetry snapshot to disk.
    pub fn save(&self, path: &PathBuf) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {}", e))?;
        }
        let report = self.report();
        let json =
            serde_json::to_string_pretty(&report).map_err(|e| format!("serialize: {}", e))?;
        std::fs::write(path, &json).map_err(|e| format!("write telemetry: {}", e))
    }

    /// Load telemetry from disk. Returns empty if file doesn't exist.
    pub fn load(path: &PathBuf) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let data =
            std::fs::read_to_string(path).map_err(|e| format!("read telemetry: {}", e))?;
        let report: HarnessTelemetryReport =
            serde_json::from_str(&data).map_err(|e| format!("parse telemetry: {}", e))?;

        let telemetry = Self::new();
        {
            let mut inner = telemetry.inner.lock().unwrap();
            inner.tool_calls = report.tool_calls;
            inner.tool_failures = report.tool_failures;
            inner.policy_violations = report.policy_violations;
            inner.task_retries = report.task_retries;
        }
        Ok(telemetry)
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_report() {
        let t = HarnessTelemetry::new();
        let report = t.report();
        assert_eq!(report.total_tool_calls, 0);
        assert_eq!(report.total_tool_failures, 0);
        assert_eq!(report.total_retries, 0);
    }

    #[test]
    fn test_record_tool_calls() {
        let t = HarnessTelemetry::new();
        t.record_tool_call_result("bash", true);
        t.record_tool_call_result("bash", true);
        t.record_tool_call_result("bash", false);
        t.record_tool_call_result("search", true);

        let report = t.report();
        assert_eq!(report.total_tool_calls, 4);
        assert_eq!(report.total_tool_failures, 1);
        assert_eq!(report.unique_tools, 2);

        let bash_failures = report.tool_failures.get("bash").copied().unwrap_or(0);
        assert_eq!(bash_failures, 1);
    }

    #[test]
    fn test_failure_rate() {
        let t = HarnessTelemetry::new();
        t.record_tool_call_result("bash", true);
        t.record_tool_call_result("bash", false);
        t.record_tool_call_result("bash", true);

        let rate = t.failure_rate("bash").unwrap();
        assert!((rate - 1.0 / 3.0).abs() < 0.01);

        // Unknown tool
        assert!(t.failure_rate("nonexistent").is_none());
    }

    #[test]
    fn test_most_failing_tool() {
        let t = HarnessTelemetry::new();
        t.record_tool_call_result("bash", false);
        t.record_tool_call_result("search", false);
        t.record_tool_call_result("search", false);

        let (name, count) = t.most_failing_tool().unwrap();
        assert_eq!(name, "search");
        assert_eq!(count, 2);
    }

    #[test]
    fn test_policy_violations() {
        let t = HarnessTelemetry::new();
        t.record_policy_violation("sha256:strict-v1");
        t.record_policy_violation("sha256:strict-v1");
        t.record_policy_violation("sha256:loose-v2");

        let (hash, count) = t.most_violated_policy().unwrap();
        assert_eq!(hash, "sha256:strict-v1");
        assert_eq!(count, 2);
    }

    #[test]
    fn test_task_retries() {
        let t = HarnessTelemetry::new();
        t.record_retry("enso-attestation");
        t.record_retry("enso-attestation");
        t.record_retry("enso-attestation");
        t.record_retry("sandbox-build");

        let report = t.report();
        assert_eq!(report.total_retries, 4);
        assert_eq!(report.unique_task_types, 2);
    }

    #[test]
    fn test_save_and_load() {
        let tmp = std::env::temp_dir().join("arli_test_telemetry.json");
        let _ = std::fs::remove_file(&tmp);

        let t = HarnessTelemetry::new();
        t.record_tool_call_result("bash", false);
        t.record_tool_call_result("bash", true);
        t.record_policy_violation("sha256:test");

        t.save(&tmp).unwrap();

        let loaded = HarnessTelemetry::load(&tmp).unwrap();
        let report = loaded.report();
        assert_eq!(report.total_tool_calls, 2);
        assert_eq!(report.total_tool_failures, 1);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_report_json() {
        let t = HarnessTelemetry::new();
        t.record_tool_call_result("bash", true);

        let json = t.report_json().unwrap();
        assert!(json.contains("total_tool_calls"));
        assert!(json.contains("\"bash\""));
    }

    #[test]
    fn test_reset() {
        let t = HarnessTelemetry::new();
        t.record_tool_call_result("bash", true);
        assert_eq!(t.report().total_tool_calls, 1);

        t.reset();
        assert_eq!(t.report().total_tool_calls, 0);
    }

    #[test]
    fn test_thread_safety() {
        let t = std::sync::Arc::new(HarnessTelemetry::new());
        let mut handles = vec![];

        for i in 0..10 {
            let t_clone = t.clone();
            handles.push(std::thread::spawn(move || {
                t_clone.record_tool_call_result(&format!("tool_{}", i % 3), i % 2 == 0);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let report = t.report();
        assert_eq!(report.total_tool_calls, 10);
        assert_eq!(report.unique_tools, 3); // tool_0, tool_1, tool_2
    }
}
