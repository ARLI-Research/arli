//! Guardrail — Pre-Reply safety checkpoint following AgentDoG 1.5 design.
//!
//! Two modes:
//! - PolicyBased: uses existing PolicyEngine rules to classify risks
//! - LlmJudge: sends trajectory to an LLM for safety evaluation (costly, accurate)
//!
//! Intercepts before the agent's final response is delivered.
//! Decision: Safe (deliver) or Unsafe (block + replace with warning).

use std::sync::Arc;

use crate::providers::{ChatMessage, LlmResponseContent, Provider, ToolSchema};
use crate::safety::{ExecutionSetting, FailureMode, RealWorldHarm, RiskSource, SafetyClassification};
use tracing::{info, warn};

/// Guardrail decision after evaluating a trajectory.
#[derive(Debug, Clone)]
pub enum GuardDecision {
    /// Safe — deliver the original response
    Safe,
    /// Unsafe — block and replace with warning
    Unsafe {
        /// The safety classification explaining why
        classification: SafetyClassification,
        /// Replacement message to show the user
        replacement: String,
    },
}

impl GuardDecision {
    pub fn is_safe(&self) -> bool {
        matches!(self, GuardDecision::Safe)
    }
}

/// Guardrail mode.
pub enum GuardMode {
    /// Use PolicyEngine rules only (fast, no API cost)
    PolicyBased,
    /// Use an LLM to evaluate the trajectory (accurate, costs tokens)
    LlmJudge {
        provider: Arc<dyn Provider>,
        model_name: String,
    },
    /// Policy first, then LLM if uncertain
    Hybrid {
        provider: Arc<dyn Provider>,
        model_name: String,
    },
}

impl std::fmt::Debug for GuardMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GuardMode::PolicyBased => write!(f, "PolicyBased"),
            GuardMode::LlmJudge { model_name, .. } => write!(f, "LlmJudge({})", model_name),
            GuardMode::Hybrid { model_name, .. } => write!(f, "Hybrid({})", model_name),
        }
    }
}

impl Clone for GuardMode {
    fn clone(&self) -> Self {
        match self {
            GuardMode::PolicyBased => GuardMode::PolicyBased,
            GuardMode::LlmJudge { provider, model_name } => GuardMode::LlmJudge {
                provider: Arc::clone(provider),
                model_name: model_name.clone(),
            },
            GuardMode::Hybrid { provider, model_name } => GuardMode::Hybrid {
                provider: Arc::clone(provider),
                model_name: model_name.clone(),
            },
        }
    }
}

/// The guardrail — evaluates agent trajectories before delivery.
pub struct Guardrail {
    mode: GuardMode,
    setting: ExecutionSetting,
    /// Minimum confidence threshold for LLM decisions (0.0-1.0)
    confidence_threshold: f64,
    /// Maximum tokens to send to LLM judge
    max_judge_tokens: usize,
}

impl Guardrail {
    /// Create a policy-based guardrail (no LLM cost).
    pub fn policy_based(setting: ExecutionSetting) -> Self {
        Self {
            mode: GuardMode::PolicyBased,
            setting,
            confidence_threshold: 0.7,
            max_judge_tokens: 4000,
        }
    }

    /// Create an LLM-as-judge guardrail.
    pub fn llm_judge(
        provider: Arc<dyn Provider>,
        model_name: impl Into<String>,
        setting: ExecutionSetting,
    ) -> Self {
        Self {
            mode: GuardMode::LlmJudge {
                provider,
                model_name: model_name.into(),
            },
            setting,
            confidence_threshold: 0.7,
            max_judge_tokens: 4000,
        }
    }

    /// Create a hybrid guardrail: policy first, LLM if uncertain.
    pub fn hybrid(
        provider: Arc<dyn Provider>,
        model_name: impl Into<String>,
        setting: ExecutionSetting,
    ) -> Self {
        Self {
            mode: GuardMode::Hybrid {
                provider,
                model_name: model_name.into(),
            },
            setting,
            confidence_threshold: 0.7,
            max_judge_tokens: 4000,
        }
    }

    /// Evaluate a trajectory and return a guard decision.
    ///
    /// `messages` is the full conversation history.
    /// `final_reply` is the text the agent is about to send.
    /// `tool_history` is a summary of tool calls made in this trajectory.
    pub async fn evaluate(
        &self,
        messages: &[ChatMessage],
        final_reply: &str,
        tool_history: &[ToolCallRecord],
    ) -> GuardDecision {
        match &self.mode {
            GuardMode::PolicyBased => self.evaluate_policy(messages, final_reply, tool_history),
            GuardMode::LlmJudge { provider, .. } => {
                self.evaluate_llm(provider, messages, final_reply, tool_history).await
            }
            GuardMode::Hybrid { provider, .. } => {
                let policy_decision = self.evaluate_policy(messages, final_reply, tool_history);
                match policy_decision {
                    GuardDecision::Safe => {
                        // Policy says safe — double-check with LLM if tool history is complex
                        if tool_history.len() > 5 {
                            self.evaluate_llm(provider, messages, final_reply, tool_history).await
                        } else {
                            GuardDecision::Safe
                        }
                    }
                    unsafe_decision => unsafe_decision,
                }
            }
        }
    }

    /// Policy-based evaluation: scan for known risk patterns.
    fn evaluate_policy(
        &self,
        _messages: &[ChatMessage],
        final_reply: &str,
        tool_history: &[ToolCallRecord],
    ) -> GuardDecision {
        let _categories = self.setting.risk_categories();

        // Check trading-specific risks
        if self.setting == ExecutionSetting::Trading {
            // Check for large position sizes in tool calls
            for record in tool_history {
                if record.tool_name == "execute_trade" || record.tool_name == "shell" {
                    if let Some(ref output) = record.output {
                        // Pattern: position size too large
                        if output.contains("Position size") && output.contains("exceeds") {
                            return GuardDecision::Unsafe {
                                classification: SafetyClassification::new(
                                    RiskSource::UserInput,
                                    FailureMode::ActionScopeOverreach,
                                    RealWorldHarm::FinancialHarm,
                                    "Trading position exceeds configured limits",
                                ),
                                replacement: format!(
                                    "⚠️ Trading action blocked: position exceeds configured limits.\n\n{}",
                                    output
                                ),
                            };
                        }
                        // Pattern: daily trade limit
                        if output.contains("Daily trade limit") {
                            return GuardDecision::Unsafe {
                                classification: SafetyClassification::new(
                                    RiskSource::InherentAgentFailure,
                                    FailureMode::ProceduralDeviation,
                                    RealWorldHarm::ComplianceHarm,
                                    "Daily trade limit reached",
                                ),
                                replacement: format!(
                                    "⚠️ Trading blocked: daily trade limit reached.\n\n{}",
                                    output
                                ),
                            };
                        }
                    }
                }
            }
        }

        // Check coding-specific risks
        if self.setting == ExecutionSetting::Coding {
            for record in tool_history {
                if record.tool_name == "shell" {
                    if let Some(ref cmd) = record.input {
                        let lower = cmd.to_lowercase();
                        // Destructive commands
                        if lower.contains("rm -rf /") || lower.contains("rm -rf ~")
                            || lower.contains("rm -rf .")
                        {
                            return GuardDecision::Unsafe {
                                classification: SafetyClassification::new(
                                    RiskSource::UserInput,
                                    FailureMode::DestructiveWorkspaceMutation,
                                    RealWorldHarm::FunctionalHarm,
                                    "Destructive filesystem operation detected",
                                ),
                                replacement: format!(
                                    "⚠️ Command blocked: destructive filesystem operation.\n\nCommand: `{}`",
                                    cmd
                                ),
                            };
                        }
                        // Force push to main
                        if lower.contains("push") && lower.contains("--force")
                            && (lower.contains("main") || lower.contains("master"))
                        {
                            return GuardDecision::Unsafe {
                                classification: SafetyClassification::new(
                                    RiskSource::UserInput,
                                    FailureMode::OverPrivilegedAction,
                                    RealWorldHarm::FunctionalHarm,
                                    "Force push to protected branch",
                                ),
                                replacement: format!(
                                    "⚠️ Git operation blocked: force push to protected branch.\n\nCommand: `{}`",
                                    cmd
                                ),
                            };
                        }
                    }
                }
            }
        }

        // Check gateway-specific risks
        if self.setting == ExecutionSetting::Gateway {
            // Scan final reply for potential information disclosure
            let sensitive_patterns = [
                "API_KEY", "api_key", "token", "secret", "password",
                "private key", "mnemonic",
            ];
            for pattern in &sensitive_patterns {
                if final_reply.contains(pattern) {
                    return GuardDecision::Unsafe {
                        classification: SafetyClassification::new(
                            RiskSource::PersistentMemoryContamination,
                            FailureMode::InformationDisclosure,
                            RealWorldHarm::PrivacyHarm,
                            format!("Reply contains sensitive pattern: {}", pattern),
                        ),
                        replacement: String::from(
                            "⚠️ Response blocked: contains potentially sensitive information.\n\
                             Please rephrase without exposing credentials or secrets.",
                        ),
                    };
                }
            }
        }

        // Check for harmful content in final reply
        let harmful_keywords = [
            "hack", "exploit", "vulnerability", "backdoor",
            "malware", "phishing", "bypass security",
        ];
        let lower_reply = final_reply.to_lowercase();
        for kw in &harmful_keywords {
            if lower_reply.contains(kw) {
                return GuardDecision::Unsafe {
                    classification: SafetyClassification::new(
                        RiskSource::InherentAgentFailure,
                        FailureMode::HarmfulContent,
                        RealWorldHarm::SecurityHarm,
                        format!("Reply contains potentially harmful keyword: {}", kw),
                    ),
                    replacement: String::from(
                        "⚠️ Response blocked: contains potentially harmful content.\n\
                         Please rephrase to avoid security-sensitive instructions.",
                    ),
                };
            }
        }

        // All checks passed
        GuardDecision::Safe
    }

    /// LLM-as-judge evaluation: send trajectory to a small model.
    async fn evaluate_llm(
        &self,
        provider: &Arc<dyn Provider>,
        messages: &[ChatMessage],
        final_reply: &str,
        tool_history: &[ToolCallRecord],
    ) -> GuardDecision {
        let trajectory_text = self.build_trajectory_text(messages, final_reply, tool_history);

        // Truncate to max tokens
        let truncated = if trajectory_text.len() > self.max_judge_tokens * 4 {
            // Rough estimate: 4 chars per token
            let cutoff = self.max_judge_tokens * 4;
            format!("{}... (truncated)", &trajectory_text[..cutoff])
        } else {
            trajectory_text
        };

        let judge_prompt = format!(
            "You are an AI safety guardrail. Evaluate this agent trajectory for safety risks.\n\n\
             Execution setting: {:?}\n\n\
             Classify any risks using these dimensions:\n\
             - Risk Source: where the risk comes from\n\
             - Failure Mode: how the agent failed\n\
             - Real-world Harm: what harm could result\n\n\
             TRAJECTORY:\n{}\n\n\
             FINAL REPLY:\n{}\n\n\
             Respond with JSON only:\n\
             {{\"safe\": true/false, \"risk_source\": \"...\", \"failure_mode\": \"...\",\n\
              \"real_world_harm\": \"...\", \"rationale\": \"...\", \"confidence\": 0.0-1.0}}\n\n\
             If safe, return: {{\"safe\": true, \"confidence\": 1.0}}",
            self.setting, truncated, final_reply
        );

        let judge_messages = vec![
            ChatMessage::system("You are an AI safety classifier. Output JSON only."),
            ChatMessage::user(judge_prompt),
        ];

        let empty_tools: Vec<ToolSchema> = vec![];

        match provider.chat(&judge_messages, &empty_tools).await {
            Ok(response) => {
                match response.content {
                    LlmResponseContent::Text { content } => {
                        self.parse_llm_decision(&content, final_reply)
                    }
                    _ => {
                        warn!("Guardrail LLM returned unexpected response type");
                        GuardDecision::Safe // Err on side of safety: let it through
                    }
                }
            }
            Err(e) => {
                warn!("Guardrail LLM judge failed: {}. Defaulting to Safe.", e);
                GuardDecision::Safe // Network error: don't block user
            }
        }
    }

    /// Parse the LLM judge's JSON response.
    fn parse_llm_decision(&self, content: &str, _final_reply: &str) -> GuardDecision {
        // Try to extract JSON from the response (may have markdown wrapping)
        let json_str = content
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(json) => {
                let safe = json["safe"].as_bool().unwrap_or(true);
                if safe {
                    return GuardDecision::Safe;
                }

                let confidence = json["confidence"].as_f64().unwrap_or(0.5);
                if confidence < self.confidence_threshold {
                    info!(
                        "Guardrail LLM confidence ({}) below threshold ({}), allowing",
                        confidence, self.confidence_threshold
                    );
                    return GuardDecision::Safe;
                }

                let classification = SafetyClassification::new(
                    parse_risk_source(json["risk_source"].as_str().unwrap_or("Other")),
                    parse_failure_mode(json["failure_mode"].as_str().unwrap_or("Other")),
                    parse_harm(json["real_world_harm"].as_str().unwrap_or("NoImpact")),
                    json["rationale"].as_str().unwrap_or("No rationale provided"),
                )
                .with_confidence(confidence);

                let replacement = format!(
                    "⚠️ Response blocked by safety guardrail.\n\n\
                     Reason: {}\n\
                     Risk: {:?} / {:?} / {:?}\n\
                     Confidence: {:.0}%\n\n\
                     Please rephrase your response to address this concern.",
                    classification.rationale,
                    classification.risk_source,
                    classification.failure_mode,
                    classification.real_world_harm,
                    classification.confidence * 100.0,
                );

                GuardDecision::Unsafe {
                    classification,
                    replacement,
                }
            }
            Err(e) => {
                warn!("Failed to parse guardrail LLM response: {}. Raw: {}", e, content);
                GuardDecision::Safe // Parse error: don't block
            }
        }
    }

    /// Build a formatted trajectory text for the LLM judge.
    fn build_trajectory_text(
        &self,
        messages: &[ChatMessage],
        final_reply: &str,
        tool_history: &[ToolCallRecord],
    ) -> String {
        let mut out = String::new();

        // Last N messages (keep it focused)
        let start = messages.len().saturating_sub(10);
        for msg in &messages[start..] {
            let role = format!("{:?}", msg.role);
            let content = msg.content.as_deref().unwrap_or("[tool calls]");
            out.push_str(&format!("[{}] {}\n", role, content));
        }

        // Tool call history
        if !tool_history.is_empty() {
            out.push_str("\n--- Tool Calls ---\n");
            for record in tool_history {
                out.push_str(&format!(
                    "Tool: {} | Input: {} | Output: {}\n",
                    record.tool_name,
                    record.input.as_deref().unwrap_or("N/A"),
                    record.output.as_deref().unwrap_or("N/A"),
                ));
            }
        }

        out.push_str(&format!("\n--- Final Reply ---\n{}\n", final_reply));

        out
    }
}

/// A record of a single tool call in the trajectory.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub input: Option<String>,
    pub output: Option<String>,
    pub success: bool,
}

impl ToolCallRecord {
    pub fn new(tool_name: impl Into<String>, input: Option<String>, output: Option<String>, success: bool) -> Self {
        Self {
            tool_name: tool_name.into(),
            input,
            output,
            success,
        }
    }
}

fn parse_risk_source(s: &str) -> RiskSource {
    match s {
        "UserInput" => RiskSource::UserInput,
        "DirectPromptInjection" => RiskSource::DirectPromptInjection,
        "EnvironmentObservation" => RiskSource::EnvironmentObservation,
        "PersistentMemoryContamination" => RiskSource::PersistentMemoryContamination,
        _ => RiskSource::Other(s.to_string()),
    }
}

fn parse_failure_mode(s: &str) -> FailureMode {
    match s {
        "OverPrivilegedAction" => FailureMode::OverPrivilegedAction,
        "ActionScopeOverreach" => FailureMode::ActionScopeOverreach,
        "MissingValidation" => FailureMode::MissingValidation,
        "UnsafeShellExecution" => FailureMode::UnsafeShellExecution,
        "ApprovalBypass" => FailureMode::ApprovalBypass,
        "InformationDisclosure" => FailureMode::InformationDisclosure,
        "HarmfulContent" => FailureMode::HarmfulContent,
        "DestructiveWorkspaceMutation" => FailureMode::DestructiveWorkspaceMutation,
        "FlawedReasoning" => FailureMode::FlawedReasoning,
        "ProceduralDeviation" => FailureMode::ProceduralDeviation,
        _ => FailureMode::Other(s.to_string()),
    }
}

fn parse_harm(s: &str) -> RealWorldHarm {
    match s {
        "FinancialHarm" => RealWorldHarm::FinancialHarm,
        "SecurityHarm" => RealWorldHarm::SecurityHarm,
        "PrivacyHarm" => RealWorldHarm::PrivacyHarm,
        "FunctionalHarm" => RealWorldHarm::FunctionalHarm,
        "ComplianceHarm" => RealWorldHarm::ComplianceHarm,
        "ReputationalHarm" => RealWorldHarm::ReputationalHarm,
        "PsychologicalHarm" => RealWorldHarm::PsychologicalHarm,
        "NoImpact" => RealWorldHarm::NoImpact,
        _ => RealWorldHarm::Other(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_trading_blocks_large_position() {
        let guard = Guardrail::policy_based(ExecutionSetting::Trading);

        let tool_history = vec![ToolCallRecord::new(
            "execute_trade",
            Some(r#"{"pair": "BTC-USD", "size": 100}"#.into()),
            Some("Error: Position size exceeds limit $10,000".into()),
            false,
        )];

        let messages = vec![];
        let decision = tokio_test::block_on(guard.evaluate(&messages, "Trade executed", &tool_history));

        assert!(!decision.is_safe());
    }

    #[test]
    fn test_policy_coding_blocks_rm_rf() {
        let guard = Guardrail::policy_based(ExecutionSetting::Coding);

        let tool_history = vec![ToolCallRecord::new(
            "shell",
            Some("rm -rf /".into()),
            None,
            true,
        )];

        let messages = vec![];
        let decision = tokio_test::block_on(guard.evaluate(&messages, "Done", &tool_history));

        assert!(!decision.is_safe());
    }

    #[test]
    fn test_policy_safe_trajectory() {
        let guard = Guardrail::policy_based(ExecutionSetting::General);

        let tool_history = vec![ToolCallRecord::new(
            "read_file",
            Some("Cargo.toml".into()),
            Some("[workspace]...".into()),
            true,
        )];

        let messages = vec![];
        let decision = tokio_test::block_on(guard.evaluate(&messages, "File contents", &tool_history));

        assert!(decision.is_safe());
    }

    #[test]
    fn test_policy_gateway_blocks_secrets() {
        let guard = Guardrail::policy_based(ExecutionSetting::Gateway);

        let tool_history = vec![];
        let messages = vec![];
        let decision = tokio_test::block_on(guard.evaluate(
            &messages,
            "Here is your API_KEY=*** ...",
            &tool_history,
        ));

        assert!(!decision.is_safe());
    }
}
