//! Failure Attribution — classify why a contract execution failed.
//!
//! From "Root-Cause Analysis for Multi-Agent Systems" (2025): when an agent
//! attestation is disputed, operators need to know WHY — was it the sandbox,
//! the agent, the ENSO canister, or the network? Without attribution, every
//! failure requires manual triage.
//!
//! This module classifies error messages from the oracle loop into one of
//! six categories, providing actionable information for retry/rollback decisions.

use serde::{Deserialize, Serialize};

// ============================================================================
// FAILURE CATEGORY
// ============================================================================

/// Root cause category for a contract execution failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureCategory {
    /// Verification pipeline failed — compile, lint, test, or fuzz.
    /// Agent produced broken code; retry with different approach.
    VerificationFailed,

    /// Sandbox killed the process — OOM, timeout, signal, or policy violation.
    /// Increase limits or relax sandbox policy.
    SandboxKilled,

    /// ENSO canister rejected the attestation — agent_id mismatch, signature
    /// invalid, binary hash not approved, or sandbox config mismatch.
    EnsoRejected,

    /// Network error — ICP call failed, timeout, or connection refused.
    /// Retry with backoff.
    NetworkError,

    /// Internal ARLI error — serialization, key loading, DB corruption.
    /// Requires human investigation.
    InternalError,

    /// Unclassified failure — error message didn't match any known pattern.
    /// Logged for future pattern addition.
    Unknown,
}

impl FailureCategory {
    /// Human-readable category name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::VerificationFailed => "verification_failed",
            Self::SandboxKilled => "sandbox_killed",
            Self::EnsoRejected => "enso_rejected",
            Self::NetworkError => "network_error",
            Self::InternalError => "internal_error",
            Self::Unknown => "unknown",
        }
    }

    /// Whether this failure type is retryable.
    ///
    /// Network errors and sandbox kills can be retried (increase limits).
    /// Verification failures and ENSO rejections need a different approach.
    /// Internal errors need investigation.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::NetworkError | Self::SandboxKilled)
    }

    /// Recommended action for the operator.
    pub fn recommended_action(&self) -> &'static str {
        match self {
            Self::VerificationFailed => {
                "Agent produced broken code — review agent approach, fix source, retry"
            }
            Self::SandboxKilled => {
                "Sandbox killed process — increase memory/timeout limits or relax policy"
            }
            Self::EnsoRejected => {
                "ENSO rejected attestation — check agent_id, binary hash, and sandbox config"
            }
            Self::NetworkError => {
                "Network error — retry with backoff, check ICP gateway connectivity"
            }
            Self::InternalError => {
                "Internal ARLI error — check logs, verify keypair and DB integrity"
            }
            Self::Unknown => {
                "Unknown failure — review raw error message, add pattern to classifier"
            }
        }
    }
}

impl std::fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ============================================================================
// ATTRIBUTION RESULT
// ============================================================================

/// Result of failure attribution — category + confidence + evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attribution {
    /// Classified failure category.
    pub category: FailureCategory,

    /// Confidence in the classification (0.0–1.0).
    /// 1.0 = exact match on a known pattern.
    /// < 0.5 = best guess, low confidence.
    pub confidence: f64,

    /// The specific pattern that matched (or "none" for Unknown).
    pub matched_pattern: String,

    /// The original error message (truncated to 500 chars).
    pub raw_error: String,
}

impl Attribution {
    /// Create an attribution with an exact match.
    pub fn exact(category: FailureCategory, pattern: &str, error: &str) -> Self {
        Self {
            category,
            confidence: 1.0,
            matched_pattern: pattern.to_string(),
            raw_error: truncate(error, 500),
        }
    }

    /// Create an attribution with partial confidence.
    pub fn partial(category: FailureCategory, confidence: f64, pattern: &str, error: &str) -> Self {
        Self {
            category,
            confidence: confidence.clamp(0.0, 1.0),
            matched_pattern: pattern.to_string(),
            raw_error: truncate(error, 500),
        }
    }

    /// Create an unknown attribution.
    pub fn unknown(error: &str) -> Self {
        Self {
            category: FailureCategory::Unknown,
            confidence: 0.0,
            matched_pattern: "none".to_string(),
            raw_error: truncate(error, 500),
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

// ============================================================================
// CLASSIFIER
// ============================================================================

/// Known error patterns and their matching categories.
///
/// Each entry: (pattern_substring, category, confidence).
/// Patterns are matched case-insensitively against the error message.
/// Ordered by specificity — more specific patterns first.
const ERROR_PATTERNS: &[(&str, FailureCategory, f64)] = &[
    // --- Verification failures (specific → general) ---
    (
        "Verification pipeline FAILED",
        FailureCategory::VerificationFailed,
        1.0,
    ),
    (
        "verification pipeline",
        FailureCategory::VerificationFailed,
        0.9,
    ),
    (
        "cargo check failed",
        FailureCategory::VerificationFailed,
        1.0,
    ),
    (
        "cargo test failed",
        FailureCategory::VerificationFailed,
        1.0,
    ),
    (
        "cargo clippy failed",
        FailureCategory::VerificationFailed,
        1.0,
    ),
    (
        "cargo build failed",
        FailureCategory::VerificationFailed,
        0.9,
    ),
    (
        "compilation failed",
        FailureCategory::VerificationFailed,
        0.8,
    ),
    ("test failed", FailureCategory::VerificationFailed, 0.7),
    ("lint failed", FailureCategory::VerificationFailed, 0.7),
    // --- Sandbox kills ---
    ("SIGKILL", FailureCategory::SandboxKilled, 1.0),
    ("out of memory", FailureCategory::SandboxKilled, 1.0),
    ("OOM", FailureCategory::SandboxKilled, 1.0),
    ("killed", FailureCategory::SandboxKilled, 0.9),
    ("timeout", FailureCategory::SandboxKilled, 0.8),
    ("timed out", FailureCategory::SandboxKilled, 0.9),
    ("memory limit", FailureCategory::SandboxKilled, 0.9),
    ("cpu time limit", FailureCategory::SandboxKilled, 0.9),
    (
        "sandbox policy violation",
        FailureCategory::SandboxKilled,
        1.0,
    ),
    ("seccomp", FailureCategory::SandboxKilled, 0.9),
    ("landlock", FailureCategory::SandboxKilled, 0.8),
    ("signal:", FailureCategory::SandboxKilled, 0.8),
    ("exit code 137", FailureCategory::SandboxKilled, 1.0), // SIGKILL
    ("exit code 139", FailureCategory::SandboxKilled, 1.0), // SIGSEGV
    // --- ENSO rejections ---
    (
        "agent_id does not match",
        FailureCategory::EnsoRejected,
        1.0,
    ),
    ("agent_id mismatch", FailureCategory::EnsoRejected, 1.0),
    (
        "signature verification failed",
        FailureCategory::EnsoRejected,
        1.0,
    ),
    ("invalid signature", FailureCategory::EnsoRejected, 1.0),
    (
        "binary hash not approved",
        FailureCategory::EnsoRejected,
        1.0,
    ),
    ("sandbox config hash", FailureCategory::EnsoRejected, 0.9),
    ("Not authorized", FailureCategory::EnsoRejected, 0.9),
    ("seller owner", FailureCategory::EnsoRejected, 0.9),
    ("attestation.*rejected", FailureCategory::EnsoRejected, 0.8),
    ("Disputed", FailureCategory::EnsoRejected, 0.5), // Low confidence — could be other reasons
    ("contract.*not found", FailureCategory::EnsoRejected, 0.9),
    ("contract not active", FailureCategory::EnsoRejected, 0.9),
    ("escrow", FailureCategory::EnsoRejected, 0.7),
    // --- Network errors ---
    ("connection refused", FailureCategory::NetworkError, 1.0),
    ("connection reset", FailureCategory::NetworkError, 1.0),
    ("timeout", FailureCategory::NetworkError, 0.6), // Low — could be sandbox timeout too
    ("DNS", FailureCategory::NetworkError, 0.9),
    ("TLS", FailureCategory::NetworkError, 0.9),
    ("HTTP", FailureCategory::NetworkError, 0.9),
    ("network", FailureCategory::NetworkError, 0.8),
    ("ICP", FailureCategory::NetworkError, 0.7),
    ("canister", FailureCategory::NetworkError, 0.6), // Could be ENSO rejection
    // --- Internal errors ---
    ("serialize", FailureCategory::InternalError, 0.9),
    ("deserialize", FailureCategory::InternalError, 0.9),
    ("keypair", FailureCategory::InternalError, 0.9),
    ("key file", FailureCategory::InternalError, 0.9),
    ("DB", FailureCategory::InternalError, 0.7),
    ("sqlite", FailureCategory::InternalError, 0.8),
    ("internal error", FailureCategory::InternalError, 1.0),
];

/// Classify an error message from the oracle loop.
///
/// Matches against known patterns in `ERROR_PATTERNS`.
/// Returns the highest-confidence match, or `Unknown` if nothing matches.
pub fn classify(error_message: &str) -> Attribution {
    let lower = error_message.to_lowercase();
    let mut best: Option<Attribution> = None;

    for (pattern, category, confidence) in ERROR_PATTERNS {
        if lower.contains(&pattern.to_lowercase()) {
            let attr = Attribution::partial(*category, *confidence, pattern, error_message);

            let is_better = match &best {
                None => true,
                Some(ref b) => {
                    confidence > &b.confidence
                        || (confidence == &b.confidence && pattern.len() > b.matched_pattern.len())
                }
            };

            if is_better {
                best = Some(attr);
            }
        }
    }

    best.unwrap_or_else(|| Attribution::unknown(error_message))
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_verification_failed() {
        let attr =
            classify("Verification pipeline FAILED: compile, test. Summary: ✗ compile → ✗ test");
        assert_eq!(attr.category, FailureCategory::VerificationFailed);
        assert_eq!(attr.confidence, 1.0);
        assert!(attr.category.is_retryable() == false);
    }

    #[test]
    fn test_classify_sandbox_killed_oom() {
        let attr = classify("Process killed: out of memory (OOM)");
        assert_eq!(attr.category, FailureCategory::SandboxKilled);
        assert_eq!(attr.confidence, 1.0);
        assert!(attr.category.is_retryable());
    }

    #[test]
    fn test_classify_sandbox_killed_sigkill() {
        let attr = classify("Command exited with signal: SIGKILL");
        assert_eq!(attr.category, FailureCategory::SandboxKilled);
        assert_eq!(attr.confidence, 1.0);
    }

    #[test]
    fn test_classify_sandbox_killed_exit_137() {
        let attr = classify("Process exited with exit code 137");
        assert_eq!(attr.category, FailureCategory::SandboxKilled);
        assert_eq!(attr.confidence, 1.0);
    }

    #[test]
    fn test_classify_enso_rejected_agent_id() {
        let attr = classify("Attestation agent_id does not match contract provider");
        assert_eq!(attr.category, FailureCategory::EnsoRejected);
        assert_eq!(attr.confidence, 1.0);
    }

    #[test]
    fn test_classify_enso_rejected_signature() {
        let attr = classify("ENSO: signature verification failed for attestation");
        assert_eq!(attr.category, FailureCategory::EnsoRejected);
        assert_eq!(attr.confidence, 1.0);
    }

    #[test]
    fn test_classify_enso_rejected_disputed() {
        let attr = classify("Disputed: ENSO verification checks failed");
        assert_eq!(attr.category, FailureCategory::EnsoRejected);
        // "Disputed" is 0.5 confidence — low because it's ambiguous
        assert_eq!(attr.confidence, 0.5);
    }

    #[test]
    fn test_classify_network_error() {
        let attr = classify("ICP call failed: connection refused");
        assert_eq!(attr.category, FailureCategory::NetworkError);
        assert_eq!(attr.confidence, 1.0);
    }

    #[test]
    fn test_classify_network_error_timeout() {
        let attr = classify("Request timed out after 30s");
        // "timeout" matches NetworkError (0.6) AND SandboxKilled (0.8)
        // SandboxKilled wins due to higher confidence
        assert_eq!(attr.category, FailureCategory::SandboxKilled);
        assert!(attr.confidence >= 0.8);
    }

    #[test]
    fn test_classify_internal_serialization() {
        let attr = classify("Failed to serialize attestation JSON");
        assert_eq!(attr.category, FailureCategory::InternalError);
        assert_eq!(attr.confidence, 0.9);
    }

    #[test]
    fn test_classify_unknown() {
        let attr = classify("Something completely unexpected happened here");
        assert_eq!(attr.category, FailureCategory::Unknown);
        assert_eq!(attr.confidence, 0.0);
        assert_eq!(attr.matched_pattern, "none");
    }

    #[test]
    fn test_classify_empty_string() {
        let attr = classify("");
        assert_eq!(attr.category, FailureCategory::Unknown);
    }

    #[test]
    fn test_attribution_serialization() {
        let attr = Attribution::exact(FailureCategory::SandboxKilled, "OOM", "out of memory");
        let json = serde_json::to_string(&attr).unwrap();
        let parsed: Attribution = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.category, FailureCategory::SandboxKilled);
        assert_eq!(parsed.confidence, 1.0);
    }

    #[test]
    fn test_is_retryable() {
        assert!(!FailureCategory::VerificationFailed.is_retryable());
        assert!(FailureCategory::SandboxKilled.is_retryable());
        assert!(!FailureCategory::EnsoRejected.is_retryable());
        assert!(FailureCategory::NetworkError.is_retryable());
        assert!(!FailureCategory::InternalError.is_retryable());
        assert!(!FailureCategory::Unknown.is_retryable());
    }

    #[test]
    fn test_recommended_actions_not_empty() {
        let categories = [
            FailureCategory::VerificationFailed,
            FailureCategory::SandboxKilled,
            FailureCategory::EnsoRejected,
            FailureCategory::NetworkError,
            FailureCategory::InternalError,
            FailureCategory::Unknown,
        ];
        for cat in &categories {
            assert!(!cat.recommended_action().is_empty());
        }
    }

    #[test]
    fn test_truncation() {
        let long = "a".repeat(1000);
        let attr = Attribution::unknown(&long);
        assert_eq!(attr.raw_error.len(), 503); // 500 + "..."
    }

    #[test]
    fn test_enso_not_authorized() {
        let attr = classify("Not authorized: only buyer or seller owner can create settlement");
        assert_eq!(attr.category, FailureCategory::EnsoRejected);
    }

    #[test]
    fn test_enso_binary_hash() {
        let attr = classify("Binary hash not approved for agent");
        assert_eq!(attr.category, FailureCategory::EnsoRejected);
        assert_eq!(attr.confidence, 1.0);
    }

    #[test]
    fn test_sandbox_seccomp_violation() {
        let attr = classify("Seccomp policy violation: syscall 59 (execve) blocked");
        assert_eq!(attr.category, FailureCategory::SandboxKilled);
        assert_eq!(attr.confidence, 0.9);
    }

    #[test]
    fn test_sandbox_landlock_violation() {
        let attr = classify("Landlock: access denied to /etc/passwd");
        assert_eq!(attr.category, FailureCategory::SandboxKilled);
        assert_eq!(attr.confidence, 0.8);
    }

    #[test]
    fn test_confidence_higher_wins() {
        // "timeout" is in both NetworkError (0.6) and SandboxKilled (0.8)
        let attr = classify("Process timeout after 60 seconds");
        assert_eq!(attr.category, FailureCategory::SandboxKilled);
        assert!(attr.confidence >= 0.8);
    }
}
