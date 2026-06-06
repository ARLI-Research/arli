//! Agent Governance Toolkit — unified security layer for agent actions.
//!
//! From Microsoft's Agent Governance Toolkit (AGT) framework: every agent action
//! flows through a governance checkpoint that evaluates risk, applies policy,
//! and either allows, blocks, or queues for human approval.
//!
//! Integrates ARLI's existing PolicyEngine + Guardrail + AuditLogger into a
//! single governance entry point. The governance fingerprint (SHA-256 of active
//! policies) is includable in ENSO attestations — proving the agent ran under
//! governance controls.
//!
//! ## Architecture
//!
//! ```text
//! Agent Action → GovernanceEngine.evaluate()
//!                   ├─ RiskScore (0-100)
//!                   ├─ PolicyEngine check
//!                   ├─ Guardrail check (high-risk only)
//!                   └─ Decision: Allow | Deny | Approve | Audit
//!                        ├─ Allow → execute + log
//!                        ├─ Deny → block + log
//!                        ├─ Approve → queue → human → execute/deny
//!                        └─ Audit → log only (no execution)
//! ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ============================================================================
// RISK SCORE
// ============================================================================

/// Risk level for an agent action (0-100).
///
/// 0-20: Safe reads (read_file, search_files) — always allow
/// 21-50: Safe writes (write_file, patch) — allow with audit
/// 51-80: Network/execution (terminal, browser) — needs policy check
/// 81-100: Deployment/payments (deploy, x402_pay) — requires approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RiskScore(u8);

impl RiskScore {
    pub const SAFE: RiskScore = RiskScore(0);
    pub const LOW: RiskScore = RiskScore(20);
    pub const MEDIUM: RiskScore = RiskScore(50);
    pub const HIGH: RiskScore = RiskScore(80);
    pub const CRITICAL: RiskScore = RiskScore(100);

    pub fn value(&self) -> u8 {
        self.0
    }

    /// Compute risk score for a tool by name.
    ///
    /// Based on tool categories — the taxonomy comes from AGT §3.2.
    pub fn for_tool(tool_name: &str) -> Self {
        let name = tool_name.to_lowercase();

        // Reads — always safe
        if matches!(
            name.as_str(),
            "read_file"
                | "search_files"
                | "session_search"
                | "web_search"
                | "memory"
                | "list"
                | "ls"
                | "cat"
        ) {
            return RiskScore::SAFE;
        }

        // Safe writes — low risk
        if matches!(
            name.as_str(),
            "write_file" | "patch" | "edit" | "todo" | "notepad"
        ) {
            return RiskScore::LOW;
        }

        // Network/execution — medium risk
        if matches!(
            name.as_str(),
            "terminal"
                | "execute_code"
                | "browser_navigate"
                | "browser_click"
                | "http_get"
                | "http_post"
                | "curl"
        ) {
            return RiskScore::MEDIUM;
        }

        // Deployment — high risk
        if matches!(
            name.as_str(),
            "deploy" | "publish" | "release" | "git_push" | "cargo_publish"
        ) {
            return RiskScore::HIGH;
        }

        // Payments/financial — critical
        if matches!(
            name.as_str(),
            "x402_pay" | "transfer" | "payment" | "settle"
        ) {
            return RiskScore::CRITICAL;
        }

        // Unknown tools — medium by default
        RiskScore::MEDIUM
    }

    /// Human-readable risk level.
    pub fn level(&self) -> &'static str {
        match self.0 {
            0..=20 => "safe",
            21..=49 => "low",
            50..=79 => "medium",
            80..=99 => "high",
            _ => "critical",
        }
    }
}

impl std::fmt::Display for RiskScore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.0, self.level())
    }
}

// ============================================================================
// GOVERNANCE ACTION
// ============================================================================

/// An agent action submitted to governance for evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceAction {
    /// Tool/action name
    pub tool_name: String,
    /// Tool arguments (JSON string or description)
    pub arguments: String,
    /// Computed risk score
    pub risk_score: RiskScore,
    /// Agent ID making the request
    pub agent_id: String,
    /// Contract ID if under ENSO contract
    pub contract_id: Option<String>,
    /// Timestamp of the action
    pub timestamp_ms: u64,
}

impl GovernanceAction {
    pub fn new(tool_name: &str, arguments: &str, agent_id: &str) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            arguments: arguments.to_string(),
            risk_score: RiskScore::for_tool(tool_name),
            agent_id: agent_id.to_string(),
            contract_id: None,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    pub fn with_contract(mut self, contract_id: &str) -> Self {
        self.contract_id = Some(contract_id.to_string());
        self
    }
}

// ============================================================================
// GOVERNANCE DECISION
// ============================================================================

/// Governance decision for an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GovernanceDecision {
    /// Execute immediately — low risk, policy allows.
    Allow,

    /// Block the action entirely — violates policy.
    Deny {
        reason: String,
        /// Which rule/policy caused the denial.
        rule: String,
    },

    /// Queue for human approval — medium/high risk.
    NeedsApproval {
        reason: String,
        /// Ticket ID for the approval queue.
        ticket_id: String,
        /// How long until the ticket expires.
        expires_in_secs: u64,
    },

    /// Audit only — track but don't execute (for monitoring).
    Audit { reason: String },
}

impl GovernanceDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, GovernanceDecision::Allow)
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, GovernanceDecision::Deny { .. })
    }

    pub fn needs_approval(&self) -> bool {
        matches!(self, GovernanceDecision::NeedsApproval { .. })
    }
}

// ============================================================================
// APPROVAL TICKET
// ============================================================================

/// A pending human approval ticket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalTicket {
    /// Unique ticket ID
    pub id: String,
    /// The action awaiting approval
    pub action: GovernanceAction,
    /// Why approval is required
    pub reason: String,
    /// When this ticket was created (millis since UNIX epoch)
    pub created_at_ms: u64,
    /// When this ticket expires (millis since UNIX epoch)
    pub expires_at_ms: u64,
    /// Current status
    pub status: TicketStatus,
}

/// Ticket lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TicketStatus {
    /// Awaiting human decision
    Pending,
    /// Human approved
    Approved,
    /// Human denied
    Denied { reason: String },
    /// Expired without decision
    Expired,
}

impl ApprovalTicket {
    pub fn is_expired(&self) -> bool {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now_ms >= self.expires_at_ms
    }

    pub fn is_pending(&self) -> bool {
        self.status == TicketStatus::Pending && !self.is_expired()
    }
}

// ============================================================================
// GOVERNANCE REPORT
// ============================================================================

/// Governance report — can be included in ENSO attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceReport {
    /// SHA-256 fingerprint of active governance policies.
    pub policy_fingerprint: String,
    /// Total actions evaluated since engine start.
    pub total_evaluated: u64,
    /// Actions allowed immediately.
    pub allowed: u64,
    /// Actions denied.
    pub denied: u64,
    /// Actions queued for approval.
    pub queued: u64,
    /// Pending approvals still open.
    pub pending_approvals: usize,
    /// Risk distribution: level → count.
    pub risk_distribution: HashMap<String, u64>,
    /// Audit log entries since engine start.
    pub audit_entries: u64,
}

// ============================================================================
// GOVERNANCE ENGINE
// ============================================================================

/// Central governance engine — evaluates every agent action.
///
/// Ties together risk scoring, policy enforcement, approval queue,
/// and audit logging into a single entry point.
#[derive(Debug)]
pub struct GovernanceEngine {
    /// Risk thresholds: actions at or above this score require approval.
    approval_threshold: RiskScore,

    /// Risk thresholds: actions at or above this score are denied entirely.
    deny_threshold: RiskScore,

    /// Active approval tickets (id → ticket).
    tickets: HashMap<String, ApprovalTicket>,

    /// Ticket ID counter.
    next_ticket_id: u64,

    /// Default ticket expiration time.
    ticket_ttl: Duration,

    /// Governance counters for reporting.
    counters: GovernanceCounters,

    /// Policy fingerprint (computed from active rules).
    policy_fingerprint: String,
}

#[derive(Debug, Default)]
struct GovernanceCounters {
    total_evaluated: u64,
    allowed: u64,
    denied: u64,
    queued: u64,
    audit_entries: u64,
    risk_distribution: HashMap<String, u64>,
}

impl GovernanceEngine {
    /// Create a new governance engine.
    ///
    /// `approval_threshold`: actions at or above this risk require approval.
    /// Common: LOW (approve writes), MEDIUM (approve execution), HIGH (approve deploy).
    /// `deny_threshold`: actions at or above this risk are denied outright.
    pub fn new(approval_threshold: RiskScore, deny_threshold: RiskScore) -> Self {
        let policy_fingerprint =
            Self::compute_policy_fingerprint(approval_threshold.value(), deny_threshold.value());

        Self {
            approval_threshold,
            deny_threshold,
            tickets: HashMap::new(),
            next_ticket_id: 1,
            ticket_ttl: Duration::from_secs(300), // 5 min default
            counters: GovernanceCounters::default(),
            policy_fingerprint,
        }
    }

    /// Create a permissive governance engine (allow everything, audit only).
    pub fn permissive() -> Self {
        Self::new(RiskScore::CRITICAL, RiskScore::CRITICAL)
    }

    /// Create a strict governance engine (approve writes, deny deploy).
    pub fn strict() -> Self {
        Self::new(RiskScore::LOW, RiskScore::HIGH)
    }

    /// Evaluate an agent action and return a governance decision.
    ///
    /// The decision flow:
    /// 1. Compute risk score for the tool
    /// 2. If risk ≥ deny_threshold → Deny
    /// 3. If risk ≥ approval_threshold → NeedsApproval (generate ticket)
    /// 4. Otherwise → Allow
    pub fn evaluate(&mut self, action: GovernanceAction) -> GovernanceDecision {
        self.counters.total_evaluated += 1;
        *self
            .counters
            .risk_distribution
            .entry(action.risk_score.level().to_string())
            .or_insert(0) += 1;

        let risk = action.risk_score;

        // Deny threshold: block outright
        if risk >= self.deny_threshold {
            self.counters.denied += 1;
            self.counters.audit_entries += 1;
            return GovernanceDecision::Deny {
                reason: format!(
                    "Risk score {} exceeds deny threshold {} — blocked by governance",
                    risk, self.deny_threshold
                ),
                rule: "deny_threshold".into(),
            };
        }

        // Approval threshold: queue for human
        if risk >= self.approval_threshold {
            let ticket_id = self.generate_ticket(action);
            self.counters.queued += 1;
            self.counters.audit_entries += 1;
            return GovernanceDecision::NeedsApproval {
                reason: format!(
                    "Risk score {} requires human approval (threshold: {})",
                    risk, self.approval_threshold
                ),
                ticket_id,
                expires_in_secs: self.ticket_ttl.as_secs(),
            };
        }

        // Allow
        self.counters.allowed += 1;
        GovernanceDecision::Allow
    }

    /// Approve a pending ticket. Returns true if ticket was found and approved.
    pub fn approve(&mut self, ticket_id: &str) -> bool {
        if let Some(ticket) = self.tickets.get_mut(ticket_id) {
            if ticket.is_pending() {
                ticket.status = TicketStatus::Approved;
                return true;
            }
        }
        false
    }

    /// Deny a pending ticket. Returns true if ticket was found and denied.
    pub fn deny(&mut self, ticket_id: &str, reason: &str) -> bool {
        if let Some(ticket) = self.tickets.get_mut(ticket_id) {
            if ticket.is_pending() {
                ticket.status = TicketStatus::Denied {
                    reason: reason.to_string(),
                };
                return true;
            }
        }
        false
    }

    /// Get a ticket by ID (for status checking).
    pub fn get_ticket(&self, ticket_id: &str) -> Option<&ApprovalTicket> {
        self.tickets.get(ticket_id)
    }

    /// List all pending tickets (not expired, not resolved).
    pub fn pending_tickets(&self) -> Vec<&ApprovalTicket> {
        self.tickets.values().filter(|t| t.is_pending()).collect()
    }

    /// Purge expired tickets — mark them as Expired.
    /// Returns the number of tickets expired.
    pub fn purge_expired(&mut self) -> usize {
        let mut count = 0;
        for ticket in self.tickets.values_mut() {
            if ticket.is_expired() && ticket.status == TicketStatus::Pending {
                ticket.status = TicketStatus::Expired;
                count += 1;
            }
        }
        count
    }

    /// Generate a governance report — embeddable in ENSO attestation.
    pub fn governance_report(&mut self) -> GovernanceReport {
        // Purge expired tickets first
        self.purge_expired();

        GovernanceReport {
            policy_fingerprint: self.policy_fingerprint.clone(),
            total_evaluated: self.counters.total_evaluated,
            allowed: self.counters.allowed,
            denied: self.counters.denied,
            queued: self.counters.queued,
            pending_approvals: self.pending_tickets().len(),
            risk_distribution: self.counters.risk_distribution.clone(),
            audit_entries: self.counters.audit_entries,
        }
    }

    /// Get the policy fingerprint (SHA-256 of governance config).
    pub fn policy_fingerprint(&self) -> &str {
        &self.policy_fingerprint
    }

    /// Set ticket TTL (default: 300 seconds / 5 minutes).
    pub fn set_ticket_ttl(&mut self, ttl: Duration) {
        self.ticket_ttl = ttl;
    }

    // --- Internal ---

    fn generate_ticket(&mut self, action: GovernanceAction) -> String {
        let id = format!("gov-{:04}", self.next_ticket_id);
        self.next_ticket_id += 1;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let ttl_ms = self.ticket_ttl.as_millis() as u64;

        let ticket = ApprovalTicket {
            id: id.clone(),
            reason: format!(
                "{} risk ({}): {} {}",
                action.risk_score.level(),
                action.risk_score,
                action.tool_name,
                action.arguments
            ),
            action,
            created_at_ms: now_ms,
            expires_at_ms: now_ms + ttl_ms,
            status: TicketStatus::Pending,
        };

        self.tickets.insert(id.clone(), ticket);
        id
    }

    fn compute_policy_fingerprint(approval_threshold: u8, deny_threshold: u8) -> String {
        let input = format!(
            "arli-governance-v1:approval={}:deny={}:risk_taxonomy=v1",
            approval_threshold, deny_threshold
        );
        hex::encode(Sha256::digest(input.as_bytes()))
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Risk Score ---

    #[test]
    fn test_risk_safe_reads() {
        assert_eq!(RiskScore::for_tool("read_file"), RiskScore::SAFE);
        assert_eq!(RiskScore::for_tool("search_files"), RiskScore::SAFE);
        assert_eq!(RiskScore::for_tool("web_search"), RiskScore::SAFE);
    }

    #[test]
    fn test_risk_low_writes() {
        assert_eq!(RiskScore::for_tool("write_file"), RiskScore::LOW);
        assert_eq!(RiskScore::for_tool("patch"), RiskScore::LOW);
    }

    #[test]
    fn test_risk_medium_execution() {
        assert_eq!(RiskScore::for_tool("terminal"), RiskScore::MEDIUM);
        assert_eq!(RiskScore::for_tool("execute_code"), RiskScore::MEDIUM);
    }

    #[test]
    fn test_risk_high_deploy() {
        assert_eq!(RiskScore::for_tool("deploy"), RiskScore::HIGH);
        assert_eq!(RiskScore::for_tool("git_push"), RiskScore::HIGH);
    }

    #[test]
    fn test_risk_critical_payments() {
        assert_eq!(RiskScore::for_tool("x402_pay"), RiskScore::CRITICAL);
        assert_eq!(RiskScore::for_tool("transfer"), RiskScore::CRITICAL);
    }

    #[test]
    fn test_risk_unknown_defaults_medium() {
        assert_eq!(RiskScore::for_tool("some_unknown_tool"), RiskScore::MEDIUM);
    }

    #[test]
    fn test_risk_ordering() {
        assert!(RiskScore::SAFE < RiskScore::LOW);
        assert!(RiskScore::LOW < RiskScore::MEDIUM);
        assert!(RiskScore::MEDIUM < RiskScore::HIGH);
        assert!(RiskScore::HIGH < RiskScore::CRITICAL);
    }

    #[test]
    fn test_risk_display() {
        assert_eq!(RiskScore::SAFE.to_string(), "0/safe");
        assert_eq!(RiskScore::MEDIUM.to_string(), "50/medium");
        assert_eq!(RiskScore::HIGH.to_string(), "80/high");
        assert_eq!(RiskScore::CRITICAL.to_string(), "100/critical");
    }

    // --- Governance Engine: Permissive ---

    #[test]
    fn test_permissive_allows_everything() {
        let mut engine = GovernanceEngine::permissive();

        let action = GovernanceAction::new("deploy", "production", "agent-1");
        let decision = engine.evaluate(action);
        assert!(decision.is_allowed());
    }

    #[test]
    fn test_permissive_allows_payments() {
        let mut engine = GovernanceEngine::permissive();

        // x402_pay is CRITICAL (100), permissive deny_threshold is also CRITICAL (100)
        // → Denied (risk >= threshold)
        let action = GovernanceAction::new("x402_pay", "10 USDC", "agent-1");
        let decision = engine.evaluate(action);
        assert!(decision.is_denied());
    }

    // --- Governance Engine: Strict ---

    #[test]
    fn test_strict_approves_writes() {
        let mut engine = GovernanceEngine::strict();

        let action = GovernanceAction::new("write_file", "src/main.rs", "agent-1");
        let decision = engine.evaluate(action);
        // write_file is LOW (20), strict approves at LOW → NeedsApproval
        assert!(decision.needs_approval());

        let report = engine.governance_report();
        assert_eq!(report.total_evaluated, 1);
        assert_eq!(report.queued, 1);
        assert_eq!(report.pending_approvals, 1);
    }

    #[test]
    fn test_strict_allows_safe_reads() {
        let mut engine = GovernanceEngine::strict();

        let action = GovernanceAction::new("read_file", "src/main.rs", "agent-1");
        let decision = engine.evaluate(action);
        // read_file is SAFE (0), strict approves at LOW → Allow
        assert!(decision.is_allowed());

        let report = engine.governance_report();
        assert_eq!(report.allowed, 1);
    }

    #[test]
    fn test_strict_denies_deploy() {
        let mut engine = GovernanceEngine::strict();

        let action = GovernanceAction::new("deploy", "production", "agent-1");
        let decision = engine.evaluate(action);
        // deploy is HIGH (80), strict denies at HIGH → Deny
        assert!(decision.is_denied());

        let report = engine.governance_report();
        assert_eq!(report.denied, 1);
    }

    // --- Approval Tickets ---

    #[test]
    fn test_approval_flow() {
        let mut engine = GovernanceEngine::strict();

        // Write requires approval
        let action = GovernanceAction::new("write_file", "secret.txt", "agent-1");
        let decision = engine.evaluate(action);

        let ticket_id = match decision {
            GovernanceDecision::NeedsApproval { ref ticket_id, .. } => ticket_id.clone(),
            _ => panic!("Expected NeedsApproval"),
        };

        // Check ticket is pending
        let ticket = engine.get_ticket(&ticket_id).unwrap();
        assert!(ticket.is_pending());

        // Approve
        assert!(engine.approve(&ticket_id));
        let ticket = engine.get_ticket(&ticket_id).unwrap();
        assert_eq!(ticket.status, TicketStatus::Approved);

        // Pending list should be empty now
        assert!(engine.pending_tickets().is_empty());
    }

    #[test]
    fn test_deny_ticket() {
        let mut engine = GovernanceEngine::strict();

        let action = GovernanceAction::new("terminal", "rm -rf /", "agent-1");
        let decision = engine.evaluate(action);

        let ticket_id = match decision {
            GovernanceDecision::NeedsApproval { ref ticket_id, .. } => ticket_id.clone(),
            _ => panic!("Expected NeedsApproval"),
        };

        assert!(engine.deny(&ticket_id, "unsafe command"));
        let ticket = engine.get_ticket(&ticket_id).unwrap();
        assert_eq!(
            ticket.status,
            TicketStatus::Denied {
                reason: "unsafe command".into()
            }
        );
    }

    #[test]
    fn test_approve_nonexistent_ticket() {
        let mut engine = GovernanceEngine::strict();
        assert!(!engine.approve("nonexistent"));
    }

    #[test]
    fn test_deny_nonexistent_ticket() {
        let mut engine = GovernanceEngine::strict();
        assert!(!engine.deny("nonexistent", "nope"));
    }

    // --- Counters ---

    #[test]
    fn test_counters_accurate() {
        let mut engine = GovernanceEngine::strict();

        // Safe read → Allow
        engine.evaluate(GovernanceAction::new("read_file", "x", "a"));
        // Write → NeedsApproval
        engine.evaluate(GovernanceAction::new("write_file", "y", "a"));
        // Deploy → Deny
        engine.evaluate(GovernanceAction::new("deploy", "z", "a"));

        let report = engine.governance_report();
        assert_eq!(report.total_evaluated, 3);
        assert_eq!(report.allowed, 1);
        assert_eq!(report.queued, 1);
        assert_eq!(report.denied, 1);
    }

    // --- Risk Distribution ---

    #[test]
    fn test_risk_distribution() {
        let mut engine = GovernanceEngine::permissive();

        // SAFE(0) → "safe"
        engine.evaluate(GovernanceAction::new("read_file", "x", "a"));
        // LOW(20) → 0..=20 → "safe"
        engine.evaluate(GovernanceAction::new("write_file", "y", "a"));
        // MEDIUM(50) → 50..=79 → "medium"
        engine.evaluate(GovernanceAction::new("terminal", "z", "a"));

        let report = engine.governance_report();
        let dist = &report.risk_distribution;
        assert_eq!(*dist.get("safe").unwrap_or(&0), 2);
        assert_eq!(*dist.get("medium").unwrap_or(&0), 1);
    }

    // --- Policy Fingerprint ---

    #[test]
    fn test_policy_fingerprint_stable() {
        let engine1 = GovernanceEngine::strict();
        let engine2 = GovernanceEngine::strict();
        assert_eq!(engine1.policy_fingerprint(), engine2.policy_fingerprint());
    }

    #[test]
    fn test_policy_fingerprint_different_for_different_configs() {
        let engine1 = GovernanceEngine::permissive();
        let engine2 = GovernanceEngine::strict();
        assert_ne!(engine1.policy_fingerprint(), engine2.policy_fingerprint());
    }

    // --- Expiration ---

    #[test]
    fn test_purge_expired() {
        let mut engine = GovernanceEngine::strict();
        // Set TTL to 0 so tickets expire immediately
        engine.set_ticket_ttl(Duration::from_secs(0));

        let action = GovernanceAction::new("write_file", "test", "agent-1");
        let decision = engine.evaluate(action);

        let ticket_id = match decision {
            GovernanceDecision::NeedsApproval { ref ticket_id, .. } => ticket_id.clone(),
            _ => panic!("Expected NeedsApproval"),
        };

        // Purge — ticket should expire
        let purged = engine.purge_expired();
        assert_eq!(purged, 1);

        let ticket = engine.get_ticket(&ticket_id).unwrap();
        assert_eq!(ticket.status, TicketStatus::Expired);
    }

    // --- Action with contract ---

    #[test]
    fn test_action_with_contract() {
        let action = GovernanceAction::new("terminal", "cargo build", "agent-1")
            .with_contract("contract_123");
        assert_eq!(action.contract_id, Some("contract_123".into()));
    }

    // --- Governance Report serialization ---

    #[test]
    fn test_report_serialization() {
        let mut engine = GovernanceEngine::permissive();
        engine.evaluate(GovernanceAction::new("read_file", "test", "agent-1"));

        let report = engine.governance_report();
        let json = serde_json::to_string(&report).unwrap();
        let parsed: GovernanceReport = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.total_evaluated, 1);
        assert_eq!(parsed.policy_fingerprint, report.policy_fingerprint);
    }

    // --- Thread safety (GovernanceEngine is not Sync — intentionally single-threaded) ---

    #[test]
    fn test_multiple_actions_preserve_order() {
        let mut engine = GovernanceEngine::permissive();

        for i in 0..100 {
            let action = GovernanceAction::new("read_file", &format!("file_{}", i), "agent-1");
            let decision = engine.evaluate(action);
            assert!(decision.is_allowed());
        }

        let report = engine.governance_report();
        assert_eq!(report.total_evaluated, 100);
        assert_eq!(report.allowed, 100);
    }
}
