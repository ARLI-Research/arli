//! Agent safety taxonomy — 3D classification following AgentDoG 1.5.
//!
//! Three dimensions per unsafe event:
//! - RiskSource: where the risk enters the trajectory
//! - FailureMode: how the agent fails
//! - RealWorldHarm: downstream consequences
//!
//! Custom categories for ARLI execution settings:
//! - Trading: financial harm, position limits, leverage abuse
//! - Coding: workspace mutation, dependency injection, unsafe shell
//! - Gateway: cross-channel misrouting, privacy, session contamination

use serde::{Deserialize, Serialize};

/// Where the risk originates — three-dimensional decomposition from AgentDoG.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RiskSource {
    /// Malicious or jailbroken user instruction
    UserInput,
    /// Direct prompt injection into tool calls
    DirectPromptInjection,
    /// Indirect injection via environment observations
    IndirectPromptInjection,
    /// Sender or session identity ambiguity (gateway-specific)
    SenderSessionAmbiguity,
    /// Malicious or compromised environment observation
    EnvironmentObservation,
    /// Unreliable or misleading external data
    UnreliableInformation,
    /// Tool/API returning corrupted or malicious data
    CorruptedToolOutput,
    /// Repository artifact injection (coding-specific)
    RepositoryArtifactInjection,
    /// Skill/plugin supply-chain compromise (gateway-specific)
    SkillPluginCompromise,
    /// MCP dependency supply-chain compromise
    McpSupplyChain,
    /// Platform or tool vulnerability exploitation
    PlatformVulnerability,
    /// Inherent agent or LLM failures (hallucination, reasoning error)
    InherentAgentFailure,
    /// Persistent memory or session-state contamination
    PersistentMemoryContamination,
    /// Other — uncategorized risk source
    Other(String),
}

/// How the agent fails — observable behavioral failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailureMode {
    /// Over-privileged action beyond authorized scope
    OverPrivilegedAction,
    /// Missing validation of tool outputs or external data
    MissingValidation,
    /// Incorrect tool parameters (wrong pair, wrong amount, etc.)
    IncorrectToolParams,
    /// Unsafe shell or script execution
    UnsafeShellExecution,
    /// Approval bypass or missing human-in-the-loop
    ApprovalBypass,
    /// Insecure interaction with external systems
    InsecureInteraction,
    /// Procedural deviation — skipping required steps
    ProceduralDeviation,
    /// Cross-tool attack chaining
    CrossToolChaining,
    /// Policy precedence misinterpretation
    PolicyMisinterpretation,
    /// Generate harmful, offensive, or misleading content
    HarmfulContent,
    /// Action scope overreach (trading: position too large; coding: rm -rf /)
    ActionScopeOverreach,
    /// Unauthorized information disclosure
    InformationDisclosure,
    /// Cross-channel or recipient misrouting (gateway-specific)
    CrossChannelMisrouting,
    /// Unsafe unattended automation
    UnattendedAutomation,
    /// Destructive workspace mutation (coding: deleting key files)
    DestructiveWorkspaceMutation,
    /// Inefficient or wasteful execution (spam, infinite loops)
    WastefulExecution,
    /// Flawed planning or reasoning
    FlawedReasoning,
    /// Provide inaccurate, misleading, or unverified information
    UnverifiedInformation,
    /// Generation of malicious executables or scripts
    MaliciousExecutable,
    /// Choosing malicious or inappropriate tools
    ImproperToolSelection,
    /// Other — uncategorized failure mode
    Other(String),
}

/// Real-world harm — downstream consequences of the failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RealWorldHarm {
    /// Financial loss or economic damage
    FinancialHarm,
    /// Security breach or system integrity compromise
    SecurityHarm,
    /// Privacy violation or confidentiality breach
    PrivacyHarm,
    /// Reputational or interpersonal damage
    ReputationalHarm,
    /// Psychological or emotional harm
    PsychologicalHarm,
    /// Physical or health harm
    PhysicalHarm,
    /// Compliance, legal, or auditability violation
    ComplianceHarm,
    /// Information ecosystem or societal harm
    SocietalHarm,
    /// Functional or opportunity harm (service disruption)
    FunctionalHarm,
    /// Fairness, equity, or allocative harm
    FairnessHarm,
    /// Public service or resource harm
    PublicResourceHarm,
    /// No real-world impact (informational only)
    NoImpact,
    /// Other — uncategorized harm
    Other(String),
}

/// A full 3D safety classification for one event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyClassification {
    /// Where the risk came from
    pub risk_source: RiskSource,
    /// How the agent failed
    pub failure_mode: FailureMode,
    /// What real-world harm could result
    pub real_world_harm: RealWorldHarm,
    /// Human-readable explanation
    pub rationale: String,
    /// Confidence score (0.0–1.0)
    pub confidence: f64,
}

impl SafetyClassification {
    pub fn new(
        risk_source: RiskSource,
        failure_mode: FailureMode,
        real_world_harm: RealWorldHarm,
        rationale: impl Into<String>,
    ) -> Self {
        Self {
            risk_source,
            failure_mode,
            real_world_harm,
            rationale: rationale.into(),
            confidence: 1.0,
        }
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }
}

/// Execution setting — determines which risk categories are applicable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionSetting {
    /// General tool-use agent
    General,
    /// Trading agent (Hyperliquid, DEX, CEX)
    Trading,
    /// Coding agent (repo manipulation, build, deploy)
    Coding,
    /// Gateway agent (multi-platform messaging)
    Gateway,
    /// Custom setting with name
    Custom(String),
}

impl ExecutionSetting {
    /// Get recommended risk categories for this setting.
    pub fn risk_categories(&self) -> Vec<(RiskSource, FailureMode, RealWorldHarm)> {
        match self {
            ExecutionSetting::General => vec![
                (
                    RiskSource::UserInput,
                    FailureMode::OverPrivilegedAction,
                    RealWorldHarm::SecurityHarm,
                ),
                (
                    RiskSource::EnvironmentObservation,
                    FailureMode::MissingValidation,
                    RealWorldHarm::FunctionalHarm,
                ),
                (
                    RiskSource::UnreliableInformation,
                    FailureMode::UnverifiedInformation,
                    RealWorldHarm::ReputationalHarm,
                ),
            ],
            ExecutionSetting::Trading => vec![
                (
                    RiskSource::UserInput,
                    FailureMode::ActionScopeOverreach,
                    RealWorldHarm::FinancialHarm,
                ),
                (
                    RiskSource::CorruptedToolOutput,
                    FailureMode::MissingValidation,
                    RealWorldHarm::FinancialHarm,
                ),
                (
                    RiskSource::EnvironmentObservation,
                    FailureMode::IncorrectToolParams,
                    RealWorldHarm::FinancialHarm,
                ),
                (
                    RiskSource::InherentAgentFailure,
                    FailureMode::FlawedReasoning,
                    RealWorldHarm::FinancialHarm,
                ),
                (
                    RiskSource::UserInput,
                    FailureMode::ApprovalBypass,
                    RealWorldHarm::ComplianceHarm,
                ),
                (
                    RiskSource::PlatformVulnerability,
                    FailureMode::InsecureInteraction,
                    RealWorldHarm::SecurityHarm,
                ),
            ],
            ExecutionSetting::Coding => vec![
                (
                    RiskSource::RepositoryArtifactInjection,
                    FailureMode::DestructiveWorkspaceMutation,
                    RealWorldHarm::FunctionalHarm,
                ),
                (
                    RiskSource::McpSupplyChain,
                    FailureMode::InsecureInteraction,
                    RealWorldHarm::SecurityHarm,
                ),
                (
                    RiskSource::UserInput,
                    FailureMode::UnsafeShellExecution,
                    RealWorldHarm::SecurityHarm,
                ),
                (
                    RiskSource::SkillPluginCompromise,
                    FailureMode::MaliciousExecutable,
                    RealWorldHarm::SecurityHarm,
                ),
                (
                    RiskSource::EnvironmentObservation,
                    FailureMode::MissingValidation,
                    RealWorldHarm::FunctionalHarm,
                ),
                (
                    RiskSource::InherentAgentFailure,
                    FailureMode::WastefulExecution,
                    RealWorldHarm::FunctionalHarm,
                ),
            ],
            ExecutionSetting::Gateway => vec![
                (
                    RiskSource::SenderSessionAmbiguity,
                    FailureMode::CrossChannelMisrouting,
                    RealWorldHarm::PrivacyHarm,
                ),
                (
                    RiskSource::IndirectPromptInjection,
                    FailureMode::InformationDisclosure,
                    RealWorldHarm::PrivacyHarm,
                ),
                (
                    RiskSource::SkillPluginCompromise,
                    FailureMode::InsecureInteraction,
                    RealWorldHarm::SecurityHarm,
                ),
                (
                    RiskSource::PersistentMemoryContamination,
                    FailureMode::PolicyMisinterpretation,
                    RealWorldHarm::ReputationalHarm,
                ),
                (
                    RiskSource::UserInput,
                    FailureMode::UnattendedAutomation,
                    RealWorldHarm::ComplianceHarm,
                ),
                (
                    RiskSource::UserInput,
                    FailureMode::HarmfulContent,
                    RealWorldHarm::PsychologicalHarm,
                ),
            ],
            ExecutionSetting::Custom(_) => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trading_categories_exist() {
        let cats = ExecutionSetting::Trading.risk_categories();
        assert!(!cats.is_empty());
        assert!(cats
            .iter()
            .any(|(_, _, h)| *h == RealWorldHarm::FinancialHarm));
    }

    #[test]
    fn test_coding_categories_exist() {
        let cats = ExecutionSetting::Coding.risk_categories();
        assert!(cats
            .iter()
            .any(|(s, _, _)| *s == RiskSource::RepositoryArtifactInjection));
    }

    #[test]
    fn test_classification_serialization() {
        let c = SafetyClassification::new(
            RiskSource::UserInput,
            FailureMode::ActionScopeOverreach,
            RealWorldHarm::FinancialHarm,
            "Position size exceeds trading limits",
        );
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("UserInput"));
        assert!(json.contains("ActionScopeOverreach"));
        assert!(json.contains("FinancialHarm"));
    }
}
