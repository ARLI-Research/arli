//! Quality Critic — LLM-based review of agent responses before delivery.
//!
//! From "Critique-Driven Feedback for Code Agents" (2025): a second LLM pass
//! reviews the agent's output for correctness, completeness, and clarity —
//! catching hallucinations, incomplete answers, and confusing responses before
//! they reach the user or ENSO.
//!
//! Unlike the safety guardrail (which blocks dangerous content), the quality
//! critic provides suggestions for improvement. The agent can then revise its
//! response based on the critique.
//!
//! Designed to be lightweight — uses a cheap model (e.g., deepseek-chat) and
//! only activates for high-stakes responses (ENSO attestations, trading signals).

use serde::{Deserialize, Serialize};

// ============================================================================
// CRITIQUE RESULT
// ============================================================================

/// Result of a quality critique pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CritiqueResult {
    /// Overall quality score (1–10, 10 = perfect).
    pub score: u8,

    /// Whether the response is acceptable as-is (score >= threshold).
    pub acceptable: bool,

    /// List of issues found.
    pub issues: Vec<CritiqueIssue>,

    /// Summary of the critique (1–2 sentences).
    pub summary: String,

    /// Whether the critic suggests revision.
    pub needs_revision: bool,
}

/// A single issue found by the critic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CritiqueIssue {
    /// Severity: "error", "warning", "info".
    pub severity: String,

    /// What aspect is problematic.
    pub category: String,

    /// Human-readable description of the issue.
    pub description: String,

    /// Suggested fix (if applicable).
    pub suggestion: Option<String>,
}

impl CritiqueResult {
    /// Create a passing critique (no issues).
    pub fn pass(score: u8, summary: &str) -> Self {
        Self {
            score: score.min(10),
            acceptable: true,
            issues: vec![],
            summary: summary.to_string(),
            needs_revision: false,
        }
    }

    /// Create a failing critique (needs revision).
    pub fn fail(score: u8, issues: Vec<CritiqueIssue>, summary: &str) -> Self {
        let needs_revision = !issues.is_empty() || score < 6;
        Self {
            score: score.min(10),
            acceptable: false,
            issues,
            summary: summary.to_string(),
            needs_revision,
        }
    }
}

// ============================================================================
// CRITIQUE CATEGORIES
// ============================================================================

/// Known critique categories for agent responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CritiqueCategory {
    /// Response is factually incorrect or hallucinated.
    FactualError,
    /// Response is incomplete — missing key information.
    Incomplete,
    /// Response is correct but poorly structured or confusing.
    Clarity,
    /// Response is too verbose or too terse.
    Length,
    /// Response makes unsafe or risky claims (borderline safety).
    SafetyConcern,
    /// Response is good but could be stronger.
    Improvement,
}

impl CritiqueCategory {
    pub fn name(&self) -> &'static str {
        match self {
            Self::FactualError => "factual_error",
            Self::Incomplete => "incomplete",
            Self::Clarity => "clarity",
            Self::Length => "length",
            Self::SafetyConcern => "safety_concern",
            Self::Improvement => "improvement",
        }
    }

    pub fn severity(&self) -> &'static str {
        match self {
            Self::FactualError | Self::SafetyConcern => "error",
            Self::Incomplete | Self::Clarity => "warning",
            Self::Length | Self::Improvement => "info",
        }
    }
}

// ============================================================================
// HEURISTIC CRITIC (no LLM required)
// ============================================================================

/// A fast, rule-based critic that catches obvious quality issues without
/// calling an LLM. Used as a first pass before the LLM critic.
pub struct HeuristicCritic {
    /// Minimum acceptable response length in characters.
    pub min_length: usize,
    /// Maximum acceptable response length in characters.
    pub max_length: usize,
    /// Score threshold for "acceptable" (1–10).
    pub score_threshold: u8,
}

impl Default for HeuristicCritic {
    fn default() -> Self {
        Self {
            min_length: 20,
            max_length: 8000,
            score_threshold: 6,
        }
    }
}

impl HeuristicCritic {
    /// Quick heuristic check — catches obvious issues without LLM cost.
    ///
    /// Checks: emptiness, too short, too long, common hallucination markers.
    pub fn critique(&self, response: &str) -> CritiqueResult {
        let mut issues = Vec::new();
        let mut score: u8 = 8; // Start at 8, deduct for issues

        // Empty response
        if response.trim().is_empty() {
            return CritiqueResult::fail(
                1,
                vec![CritiqueIssue {
                    severity: "error".into(),
                    category: CritiqueCategory::Incomplete.name().into(),
                    description: "Response is empty".into(),
                    suggestion: Some("Generate a non-empty response".into()),
                }],
                "Empty response — cannot deliver",
            );
        }

        // Too short
        if response.len() < self.min_length {
            score = score.saturating_sub(3);
            issues.push(CritiqueIssue {
                severity: "warning".into(),
                category: CritiqueCategory::Length.name().into(),
                description: format!(
                    "Response is very short ({} chars, min {})",
                    response.len(),
                    self.min_length
                ),
                suggestion: Some("Expand the response with more detail".into()),
            });
        }

        // Too long
        if response.len() > self.max_length {
            score = score.saturating_sub(1);
            issues.push(CritiqueIssue {
                severity: "info".into(),
                category: CritiqueCategory::Length.name().into(),
                description: format!(
                    "Response is very long ({} chars, max {})",
                    response.len(),
                    self.max_length
                ),
                suggestion: Some("Consider trimming or summarizing".into()),
            });
        }

        // Hallucination markers
        let lower = response.to_lowercase();
        let hallucination_phrases = [
            "as an ai",
            "i cannot",
            "i'm unable",
            "i apologize",
            "i don't have",
            "i do not have",
            "unfortunately",
            "i am not able",
            "i'm not able",
            "as a language model",
        ];

        let mut hallucination_count = 0;
        for phrase in &hallucination_phrases {
            if lower.contains(phrase) {
                hallucination_count += 1;
            }
        }

        if hallucination_count >= 3 {
            score = score.saturating_sub(3);
            issues.push(CritiqueIssue {
                severity: "warning".into(),
                category: CritiqueCategory::SafetyConcern.name().into(),
                description: format!(
                    "Response contains {} refusal/hallucination markers",
                    hallucination_count
                ),
                suggestion: Some(
                    "Rewrite to be direct and confident; avoid AI-disclaimer language".into(),
                ),
            });
        } else if hallucination_count >= 1 {
            score = score.saturating_sub(1);
            issues.push(CritiqueIssue {
                severity: "info".into(),
                category: CritiqueCategory::Clarity.name().into(),
                description: "Response contains mild AI-disclaimer language".into(),
                suggestion: Some("Consider removing 'as an AI' type phrasing".into()),
            });
        }

        // Repetition check: same sentence repeated
        let sentences: Vec<&str> = response.split(&['.', '!', '?'][..]).collect();
        let mut seen = std::collections::HashSet::new();
        let mut repeats = 0;
        for s in &sentences {
            let trimmed = s.trim().to_lowercase();
            if trimmed.len() > 10 && !seen.insert(trimmed) {
                repeats += 1;
            }
        }
        if repeats >= 2 {
            score = score.saturating_sub(2);
            issues.push(CritiqueIssue {
                severity: "warning".into(),
                category: CritiqueCategory::Clarity.name().into(),
                description: format!("Response contains {} repeated sentences", repeats),
                suggestion: Some("Remove duplicate sentences".into()),
            });
        }

        // Code block presence check (for coding responses)
        let has_code_block = response.contains("```");
        let is_coding_question = lower.contains("code")
            || lower.contains("function")
            || lower.contains("bug")
            || lower.contains("error")
            || lower.contains("compile");

        if is_coding_question && !has_code_block {
            score = score.saturating_sub(1);
            issues.push(CritiqueIssue {
                severity: "info".into(),
                category: CritiqueCategory::Incomplete.name().into(),
                description: "Coding question but no code block in response".into(),
                suggestion: Some("Include a code example in ``` blocks".into()),
            });
        }

        let acceptable = score >= self.score_threshold;

        if acceptable {
            CritiqueResult::pass(score, "Heuristic check passed — no major issues")
        } else {
            let issue_count = issues.len();
            CritiqueResult::fail(
                score,
                issues,
                &format!("Heuristic check found {} issue(s)", issue_count),
            )
        }
    }
}

// ============================================================================
// LLM CRITIC (prompt-based, async)
// ============================================================================

/// System prompt template for the LLM quality critic.
const CRITIC_SYSTEM_PROMPT: &str = r#"You are a quality critic reviewing an AI agent's response. 
Your job is to find issues, not to be nice. Be direct and specific.

Evaluate the response on:
1. **Correctness**: Are there factual errors or hallucinations?
2. **Completeness**: Does it fully answer the question?
3. **Clarity**: Is it well-structured and easy to understand?
4. **Safety**: Any concerning claims or borderline content?

Output format (JSON):
{
  "score": <1-10>,
  "acceptable": <true/false>,
  "issues": [
    {
      "severity": "error|warning|info",
      "category": "factual_error|incomplete|clarity|length|safety_concern|improvement",
      "description": "<what's wrong>",
      "suggestion": "<how to fix, or null>"
    }
  ],
  "summary": "<1-2 sentence summary>"
}

Only flag real issues. A score of 8+ means the response is good as-is.
Score 6-7: minor issues, still deliverable.
Score 4-5: significant issues, should revise.
Score 1-3: unusable, must rewrite completely."#;

/// Build the user prompt for the critic.
pub fn build_critic_prompt(
    user_query: &str,
    agent_response: &str,
    context_hint: Option<&str>,
) -> String {
    let mut prompt = String::new();

    if let Some(ctx) = context_hint {
        prompt.push_str(&format!("Context: {}\n\n", ctx));
    }

    prompt.push_str(&format!("User query: {}\n\n", user_query));
    prompt.push_str(&format!(
        "Agent response to review:\n```\n{}\n```\n\n",
        agent_response
    ));
    prompt.push_str(
        "Evaluate this response. Return JSON with score, acceptable flag, issues array, and summary.",
    );

    prompt
}

/// Parse the LLM's JSON response into a CritiqueResult.
///
/// Tolerant of minor formatting issues — tries to extract JSON from markdown fences.
pub fn parse_critic_response(raw: &str) -> Result<CritiqueResult, String> {
    // Try to extract JSON from markdown code fences
    let json_str = if raw.contains("```json") {
        raw.split("```json")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or(raw)
            .trim()
    } else if raw.contains("```") {
        raw.split("```").nth(1).unwrap_or(raw).trim()
    } else {
        raw.trim()
    };

    #[derive(Deserialize)]
    struct RawCritique {
        score: u8,
        acceptable: bool,
        #[serde(default)]
        issues: Vec<RawIssue>,
        summary: String,
    }

    #[derive(Deserialize)]
    struct RawIssue {
        severity: String,
        category: String,
        description: String,
        suggestion: Option<String>,
    }

    let raw: RawCritique =
        serde_json::from_str(json_str).map_err(|e| format!("parse critic JSON: {}", e))?;

    let issues: Vec<CritiqueIssue> = raw
        .issues
        .into_iter()
        .map(|i| CritiqueIssue {
            severity: i.severity,
            category: i.category,
            description: i.description,
            suggestion: i.suggestion,
        })
        .collect();

    let needs_revision = !raw.acceptable || !issues.is_empty();

    Ok(CritiqueResult {
        score: raw.score.min(10),
        acceptable: raw.acceptable,
        issues,
        summary: raw.summary,
        needs_revision,
    })
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Heuristic Critic ---

    #[test]
    fn test_heuristic_empty_response() {
        let critic = HeuristicCritic::default();
        let result = critic.critique("");
        assert_eq!(result.score, 1);
        assert!(!result.acceptable);
        assert!(result.needs_revision);
    }

    #[test]
    fn test_heuristic_whitespace_response() {
        let critic = HeuristicCritic::default();
        let result = critic.critique("   \n  ");
        assert_eq!(result.score, 1);
    }

    #[test]
    fn test_heuristic_good_response() {
        let critic = HeuristicCritic::default();
        let response = "The bug is in src/main.rs line 42. The variable `count` is \
            used before initialization. To fix this, initialize it to 0 before the loop.\n\
            ```rust\nlet mut count = 0;\nfor item in items {\n    count += 1;\n}\n```";
        let result = critic.critique(response);
        assert!(result.score >= 6);
        assert!(result.acceptable);
        assert!(!result.needs_revision);
    }

    #[test]
    fn test_heuristic_hallucination_markers() {
        let critic = HeuristicCritic::default();
        let response = "As an AI, I cannot help with that. Unfortunately, \
            I don't have access to that information. I apologize for the inconvenience.";
        let result = critic.critique(response);
        // Should detect 3+ hallucination markers
        assert!(
            result.score < 8,
            "score should be reduced: {}",
            result.score
        );
    }

    #[test]
    fn test_heuristic_too_short() {
        let mut critic = HeuristicCritic::default();
        critic.min_length = 100;
        let result = critic.critique("OK done");
        assert!(result.score < 8);
    }

    #[test]
    fn test_heuristic_repeated_sentences() {
        let critic = HeuristicCritic::default();
        let response = "This is a test. This is a test. Something else. This is a test.";
        let result = critic.critique(response);
        assert!(result.score < 8, "score: {}", result.score);
    }

    #[test]
    fn test_heuristic_coding_question_no_code() {
        let critic = HeuristicCritic::default();
        let response = "You have a bug in your code. The function is missing a return type. \
            You should add the return type to fix the compilation error. That should resolve the issue.";
        let result = critic.critique(response);
        // Should flag: "bug" + "compile" + "function" but no code block
        assert!(result.score < 8, "score: {}", result.score);
    }

    // --- LLM Critic Parsing ---

    #[test]
    fn test_parse_critic_json_direct() {
        let raw = r#"{"score":8,"acceptable":true,"issues":[],"summary":"Good response"}"#;
        let result = parse_critic_response(raw).unwrap();
        assert_eq!(result.score, 8);
        assert!(result.acceptable);
        assert!(!result.needs_revision);
    }

    #[test]
    fn test_parse_critic_json_in_fence() {
        let raw = r#"Here is my evaluation:
```json
{"score":4,"acceptable":false,"issues":[{"severity":"error","category":"factual_error","description":"Wrong API","suggestion":"Use v2 endpoint"}],"summary":"Has errors"}
```
That's my review."#;
        let result = parse_critic_response(raw).unwrap();
        assert_eq!(result.score, 4);
        assert!(!result.acceptable);
        assert_eq!(result.issues.len(), 1);
        assert_eq!(result.issues[0].severity, "error");
        assert!(result.needs_revision);
    }

    #[test]
    fn test_parse_critic_json_in_plain_fence() {
        let raw = r#"```
{"score":7,"acceptable":true,"issues":[{"severity":"info","category":"improvement","description":"Could be more concise","suggestion":null}],"summary":"Minor issues"}
```"#;
        let result = parse_critic_response(raw).unwrap();
        assert_eq!(result.score, 7);
        assert!(result.acceptable);
        assert_eq!(result.issues.len(), 1);
        assert!(result.needs_revision); // Has issues even though acceptable
    }

    #[test]
    fn test_parse_critic_broken_json() {
        let raw = "not json at all";
        assert!(parse_critic_response(raw).is_err());
    }

    // --- CritiqueResult ---

    #[test]
    fn test_critique_pass() {
        let result = CritiqueResult::pass(9, "Excellent");
        assert!(result.acceptable);
        assert!(!result.needs_revision);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn test_critique_fail() {
        let issues = vec![CritiqueIssue {
            severity: "error".into(),
            category: "factual_error".into(),
            description: "Wrong answer".into(),
            suggestion: Some("Check the API docs".into()),
        }];
        let result = CritiqueResult::fail(3, issues, "Has errors");
        assert!(!result.acceptable);
        assert!(result.needs_revision);
        assert_eq!(result.issues.len(), 1);
    }

    // --- CritiqueCategory ---

    #[test]
    fn test_category_severity() {
        assert_eq!(CritiqueCategory::FactualError.severity(), "error");
        assert_eq!(CritiqueCategory::SafetyConcern.severity(), "error");
        assert_eq!(CritiqueCategory::Incomplete.severity(), "warning");
        assert_eq!(CritiqueCategory::Improvement.severity(), "info");
    }

    // --- Prompt building ---

    #[test]
    fn test_build_critic_prompt() {
        let prompt = build_critic_prompt(
            "How do I fix this bug?",
            "Add a semicolon on line 42",
            Some("Rust project"),
        );
        assert!(prompt.contains("Context: Rust project"));
        assert!(prompt.contains("How do I fix this bug?"));
        assert!(prompt.contains("Add a semicolon on line 42"));
        assert!(prompt.contains("Evaluate this response"));
    }

    #[test]
    fn test_build_critic_prompt_no_context() {
        let prompt = build_critic_prompt("query", "response", None);
        assert!(!prompt.contains("Context:"));
        assert!(prompt.contains("query"));
        assert!(prompt.contains("response"));
    }
}
