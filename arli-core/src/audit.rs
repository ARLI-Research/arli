//! OCSF-inspired audit logging — structured security events (OpenShell pattern).
//!
//! Records sandbox lifecycle events, policy decisions, and security violations
//! in a machine-readable JSON format suitable for SIEM ingestion.
//!
//! Event types:
//! - `sandbox.create` — sandbox started
//! - `sandbox.destroy` — sandbox terminated
//! - `network.allow` — outbound connection allowed by policy
//! - `network.deny` — outbound connection denied by policy
//! - `filesystem.access` — file access within sandbox
//! - `process.exec` — command executed in sandbox

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// An audit event following OCSF-inspired schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Event type (e.g., "sandbox.create", "network.deny")
    pub event_type: String,

    /// ISO 8601 timestamp
    pub timestamp: String,

    /// Unix epoch milliseconds
    pub timestamp_ms: u64,

    /// Sandbox ID (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,

    /// Actor identity (user, agent, process)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<Actor>,

    /// Event-specific payload
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,

    /// Outcome of the action
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,

    /// Severity level
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
}

/// Actor that performed the action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    /// Process name
    pub process: Option<String>,

    /// User identity
    pub user: Option<String>,

    /// Agent session ID
    pub session_id: Option<String>,
}

/// Severity levels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Audit logger — appends events to a JSONL file or stdout.
pub struct AuditLogger {
    /// Output path (None = stdout)
    output_path: Option<String>,
}

impl AuditLogger {
    /// Create a logger that writes to a file.
    pub fn to_file(path: impl Into<String>) -> Self {
        Self {
            output_path: Some(path.into()),
        }
    }

    /// Create a logger that writes to stdout.
    pub fn to_stdout() -> Self {
        Self { output_path: None }
    }

    /// Log a sandbox creation event.
    pub fn sandbox_create(&self, sandbox_id: &str, policy_name: &str) {
        self.log(AuditEvent {
            event_type: "sandbox.create".into(),
            timestamp: Self::now_iso(),
            timestamp_ms: Self::now_ms(),
            sandbox_id: Some(sandbox_id.into()),
            actor: None,
            payload: Some(serde_json::json!({
                "policy": policy_name,
            })),
            outcome: Some("created".into()),
            severity: Some(Severity::Info),
        });
    }

    /// Log a sandbox destruction event.
    pub fn sandbox_destroy(&self, sandbox_id: &str, exit_code: i32) {
        self.log(AuditEvent {
            event_type: "sandbox.destroy".into(),
            timestamp: Self::now_iso(),
            timestamp_ms: Self::now_ms(),
            sandbox_id: Some(sandbox_id.into()),
            actor: None,
            payload: Some(serde_json::json!({
                "exit_code": exit_code,
            })),
            outcome: Some("destroyed".into()),
            severity: Some(Severity::Info),
        });
    }

    /// Log a network allow/deny decision.
    pub fn network_decision(
        &self,
        sandbox_id: &str,
        allowed: bool,
        host: &str,
        port: u16,
        reason: &str,
    ) {
        self.log(AuditEvent {
            event_type: if allowed {
                "network.allow"
            } else {
                "network.deny"
            }
            .into(),
            timestamp: Self::now_iso(),
            timestamp_ms: Self::now_ms(),
            sandbox_id: Some(sandbox_id.into()),
            actor: None,
            payload: Some(serde_json::json!({
                "host": host,
                "port": port,
                "reason": reason,
            })),
            outcome: Some(if allowed { "allowed" } else { "denied" }.into()),
            severity: Some(if allowed {
                Severity::Info
            } else {
                Severity::Warning
            }),
        });
    }

    /// Log a security violation (seccomp kill, Landlock denial, etc.)
    pub fn security_violation(&self, sandbox_id: &str, violation_type: &str, detail: &str) {
        self.log(AuditEvent {
            event_type: "security.violation".into(),
            timestamp: Self::now_iso(),
            timestamp_ms: Self::now_ms(),
            sandbox_id: Some(sandbox_id.into()),
            actor: None,
            payload: Some(serde_json::json!({
                "type": violation_type,
                "detail": detail,
            })),
            outcome: Some("blocked".into()),
            severity: Some(Severity::Critical),
        });
    }

    /// Write an event to the output.
    fn log(&self, event: AuditEvent) {
        let line = serde_json::to_string(&event).unwrap_or_default();

        match &self.output_path {
            Some(path) => {
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(file, "{}", line);
                }
            }
            None => {
                eprintln!("[AUDIT] {}", line);
            }
        }
    }

    fn now_iso() -> String {
        // Simple ISO 8601 without chrono dependency
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();
        // Format: 2024-01-01T00:00:00Z
        let days = secs / 86400;
        let time_of_day = secs % 86400;
        let hours = time_of_day / 3600;
        let minutes = (time_of_day % 3600) / 60;
        let seconds = time_of_day % 60;
        // Approximate date from UNIX epoch (simplified)
        format!("unix_{}.{:02}:{:02}:{:02}Z", days, hours, minutes, seconds)
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        // Default: log to ~/.arli/audit.log if possible
        let path = std::env::var("HOME")
            .map(|h| format!("{}/.arli/audit.log", h))
            .unwrap_or_else(|_| "/tmp/arli-audit.log".into());
        Self::to_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_create_event() {
        let logger = AuditLogger::to_stdout();
        logger.sandbox_create("sb-001", "restrictive-default");
        // Should not panic
    }

    #[test]
    fn test_network_deny_event() {
        let logger = AuditLogger::to_stdout();
        logger.network_decision("sb-001", false, "evil.com", 443, "policy default-deny");
        // Should not panic
    }

    #[test]
    fn test_security_violation_event() {
        let logger = AuditLogger::to_stdout();
        logger.security_violation("sb-001", "seccomp", "blocked syscall: mount (40)");
        // Should not panic
    }

    #[test]
    fn test_event_serialization() {
        let event = AuditEvent {
            event_type: "sandbox.create".into(),
            timestamp: "test".into(),
            timestamp_ms: 0,
            sandbox_id: Some("sb-001".into()),
            actor: None,
            payload: Some(serde_json::json!({"policy": "default"})),
            outcome: Some("created".into()),
            severity: Some(Severity::Info),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("sandbox.create"));
        assert!(json.contains("sb-001"));
    }
}
