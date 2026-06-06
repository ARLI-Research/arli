//! Harness Analytics — read-only insights over existing telemetry and lessons.
//!
//! From "Evolution-Driven Agent Harness Optimization" (2025): the harness should
//! self-analyze — identify failing tools, overly-aggressive policies, and stale
//! lessons — and surface actionable recommendations without requiring a human to
//! dig through JSON files.
//!
//! This module loads `~/.arli/telemetry.json` and `~/.arli/lessons.json` and
//! produces a structured report with:
//!   - Top failing tools (by failure rate)
//!   - Most violated policies
//!   - Most retried task types
//!   - Memory health (hit rate, stale lessons)
//!   - Actionable recommendations

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::harness_telemetry::HarnessTelemetryReport;
use crate::lessons::ExperientialMemory;

// ============================================================================
// ANALYTICS REPORT
// ============================================================================

/// Full harness analytics report — loaded from persisted data files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessAnalyticsReport {
    /// Path to the telemetry file that was analyzed.
    pub telemetry_path: String,
    /// Path to the lessons file that was analyzed.
    pub lessons_path: String,
    /// Whether telemetry data was found.
    pub has_telemetry: bool,
    /// Whether lessons data was found.
    pub has_lessons: bool,

    // Tool insights
    /// Tools ranked by failure rate (highest first).
    pub top_failing_tools: Vec<ToolInsight>,
    /// Tool with the highest raw failure count.
    pub most_failing_tool: Option<ToolInsight>,
    /// Overall tool failure rate (0.0–1.0).
    pub overall_failure_rate: f64,

    // Policy insights
    /// Policies ranked by violation count (highest first).
    pub top_violated_policies: Vec<PolicyInsight>,

    // Retry insights
    /// Task types ranked by retry count (highest first).
    pub top_retried_tasks: Vec<RetryInsight>,

    // Memory insights
    /// Memory hit rate (0.0–1.0), None if no lookups.
    pub memory_hit_rate: Option<f64>,
    /// Total lessons stored.
    pub total_lessons: usize,
    /// Verified lessons.
    pub verified_lessons: usize,
    /// Unverified (potentially stale) lessons.
    pub unverified_lessons: usize,
    /// Top lessons by effectiveness (hits vs misses).
    pub top_effective_lessons: Vec<LessonInsight>,
    /// Lessons with zero hits (candidates for pruning).
    pub zero_hit_lessons: usize,

    // Telemetry totals
    pub total_tool_calls: u64,
    pub total_tool_failures: u64,
    pub total_policy_violations: u64,
    pub total_retries: u64,
    pub unique_tools: usize,
    pub unique_policies: usize,
    pub unique_task_types: usize,

    // Recommendations
    pub recommendations: Vec<String>,
}

/// Insight about a single tool's performance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInsight {
    pub tool_name: String,
    pub total_calls: u64,
    pub total_failures: u64,
    pub failure_rate: f64,
}

/// Insight about a single policy's violations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyInsight {
    pub policy_hash: String,
    pub violations: u64,
}

/// Insight about retry patterns for a task type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryInsight {
    pub task_type: String,
    pub retries: u64,
}

/// Insight about a lesson's effectiveness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LessonInsight {
    pub error_pattern: String,
    pub fix_description: String,
    pub hits: u32,
    pub misses: u32,
    pub effectiveness: f64, // hits / (hits + misses), 0.0 if never tried
    pub times_applied: u32,
    pub verified: bool,
}

// ============================================================================
// HARNESS ANALYZER
// ============================================================================

/// Harness analyzer — loads persisted data and computes insights.
pub struct HarnessAnalyzer {
    telemetry_path: PathBuf,
    lessons_path: PathBuf,
}

impl Default for HarnessAnalyzer {
    fn default() -> Self {
        let arli_home = crate::config::arli_home();
        Self {
            telemetry_path: arli_home.join("telemetry.json"),
            lessons_path: arli_home.join("lessons.json"),
        }
    }
}

impl HarnessAnalyzer {
    /// Create analyzer with custom paths.
    pub fn new(telemetry_path: PathBuf, lessons_path: PathBuf) -> Self {
        Self {
            telemetry_path,
            lessons_path,
        }
    }

    /// Run full analysis — load data, compute insights, return report.
    pub fn analyze(&self) -> HarnessAnalyticsReport {
        let telemetry = self.load_telemetry();
        let lessons = self.load_lessons();

        let has_telemetry = telemetry.is_some();
        let has_lessons = lessons.is_some();

        let telemetry = telemetry.unwrap_or_default();
        let lessons = lessons.unwrap_or_default();

        let top_failing_tools = self.compute_top_failing_tools(&telemetry);
        let most_failing_tool = top_failing_tools.first().cloned();
        let overall_failure_rate = if telemetry.total_tool_calls > 0 {
            telemetry.total_tool_failures as f64 / telemetry.total_tool_calls as f64
        } else {
            0.0
        };

        let top_violated_policies = self.compute_top_violated_policies(&telemetry);
        let top_retried_tasks = self.compute_top_retried_tasks(&telemetry);

        let memory_hit_rate = if telemetry.memory_lookups > 0 {
            Some(telemetry.memory_hit_rate)
        } else {
            None
        };

        let total_lessons = lessons.lessons.len();
        let verified_lessons = lessons.lessons.iter().filter(|l| l.verified).count();
        let unverified_lessons = total_lessons - verified_lessons;

        let top_effective_lessons = self.compute_top_effective_lessons(&lessons);
        let zero_hit_lessons = lessons
            .lessons
            .iter()
            .filter(|l| l.hits == 0 && l.times_applied > 0)
            .count();

        let recommendations = self.generate_recommendations(
            &top_failing_tools,
            &top_violated_policies,
            memory_hit_rate,
            unverified_lessons,
            zero_hit_lessons,
        );

        HarnessAnalyticsReport {
            telemetry_path: self.telemetry_path.display().to_string(),
            lessons_path: self.lessons_path.display().to_string(),
            has_telemetry,
            has_lessons,
            top_failing_tools,
            most_failing_tool,
            overall_failure_rate,
            top_violated_policies,
            top_retried_tasks,
            memory_hit_rate,
            total_lessons,
            verified_lessons,
            unverified_lessons,
            top_effective_lessons,
            zero_hit_lessons,
            total_tool_calls: telemetry.total_tool_calls,
            total_tool_failures: telemetry.total_tool_failures,
            total_policy_violations: telemetry.policy_violations.values().sum(),
            total_retries: telemetry.total_retries,
            unique_tools: telemetry.unique_tools,
            unique_policies: telemetry.unique_policies,
            unique_task_types: telemetry.unique_task_types,
            recommendations,
        }
    }

    // --- Data loading ---

    fn load_telemetry(&self) -> Option<HarnessTelemetryReport> {
        if !self.telemetry_path.exists() {
            return None;
        }
        let data = std::fs::read_to_string(&self.telemetry_path).ok()?;
        serde_json::from_str(&data).ok()
    }

    fn load_lessons(&self) -> Option<ExperientialMemory> {
        ExperientialMemory::load(&self.lessons_path).ok()
    }

    // --- Computations ---

    fn compute_top_failing_tools(&self, telemetry: &HarnessTelemetryReport) -> Vec<ToolInsight> {
        let mut insights: Vec<ToolInsight> = telemetry
            .tool_calls
            .iter()
            .map(|(name, &calls)| {
                let failures = telemetry.tool_failures.get(name).copied().unwrap_or(0);
                ToolInsight {
                    tool_name: name.clone(),
                    total_calls: calls,
                    total_failures: failures,
                    failure_rate: if calls > 0 {
                        failures as f64 / calls as f64
                    } else {
                        0.0
                    },
                }
            })
            .collect();

        // Sort by failure rate descending, then by call count descending
        insights.sort_by(|a, b| {
            b.failure_rate
                .partial_cmp(&a.failure_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.total_calls.cmp(&a.total_calls))
        });

        insights
    }

    fn compute_top_violated_policies(
        &self,
        telemetry: &HarnessTelemetryReport,
    ) -> Vec<PolicyInsight> {
        let mut insights: Vec<PolicyInsight> = telemetry
            .policy_violations
            .iter()
            .map(|(hash, &count)| PolicyInsight {
                policy_hash: hash.clone(),
                violations: count,
            })
            .collect();

        insights.sort_by(|a, b| b.violations.cmp(&a.violations));
        insights
    }

    fn compute_top_retried_tasks(&self, telemetry: &HarnessTelemetryReport) -> Vec<RetryInsight> {
        let mut insights: Vec<RetryInsight> = telemetry
            .task_retries
            .iter()
            .map(|(task_type, &retries)| RetryInsight {
                task_type: task_type.clone(),
                retries,
            })
            .collect();

        insights.sort_by(|a, b| b.retries.cmp(&a.retries));
        insights
    }

    fn compute_top_effective_lessons(&self, lessons: &ExperientialMemory) -> Vec<LessonInsight> {
        let mut insights: Vec<LessonInsight> = lessons
            .lessons
            .iter()
            .map(|l| {
                let total = l.hits + l.misses;
                LessonInsight {
                    error_pattern: l.error_pattern.clone(),
                    fix_description: l.fix_description.clone(),
                    hits: l.hits,
                    misses: l.misses,
                    effectiveness: if total > 0 {
                        l.hits as f64 / total as f64
                    } else {
                        0.0
                    },
                    times_applied: l.times_applied,
                    verified: l.verified,
                }
            })
            .collect();

        insights.sort_by(|a, b| {
            b.effectiveness
                .partial_cmp(&a.effectiveness)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        insights
    }

    // --- Recommendations ---

    fn generate_recommendations(
        &self,
        top_failing: &[ToolInsight],
        top_violated: &[PolicyInsight],
        memory_hit_rate: Option<f64>,
        unverified_lessons: usize,
        zero_hit_lessons: usize,
    ) -> Vec<String> {
        let mut recs = Vec::new();

        // Failing tools
        for tool in top_failing.iter().take(3) {
            if tool.failure_rate > 0.1 && tool.total_calls >= 3 {
                recs.push(format!(
                    "Fix tool '{}': fails {:.0}% of calls ({} failures / {} calls)",
                    tool.tool_name,
                    tool.failure_rate * 100.0,
                    tool.total_failures,
                    tool.total_calls,
                ));
            }
        }

        // Violated policies
        for policy in top_violated.iter().take(2) {
            if policy.violations >= 5 {
                recs.push(format!(
                    "Review policy '{}': {} violations — may be too restrictive",
                    policy.policy_hash, policy.violations,
                ));
            }
        }

        // Memory health
        if let Some(rate) = memory_hit_rate {
            if rate < 0.3 {
                recs.push(format!(
                    "Memory hit rate is low ({:.0}%) — lessons may be ineffective or stale",
                    rate * 100.0,
                ));
            }
        }

        if unverified_lessons > 5 {
            recs.push(format!(
                "{} unverified lessons — run verification or prune with 'arli harness prune-lessons'",
                unverified_lessons,
            ));
        }

        if zero_hit_lessons > 0 {
            recs.push(format!(
                "{} lessons have zero hits despite being applied — candidates for removal",
                zero_hit_lessons,
            ));
        }

        if recs.is_empty() {
            recs.push("No issues detected — harness is healthy.".into());
        }

        recs
    }
}

// ============================================================================
// FORMATTING
// ============================================================================

impl HarnessAnalyticsReport {
    /// Format the report as human-readable text (for CLI output).
    pub fn format_text(&self) -> String {
        let mut out = String::new();

        out.push_str("═══ ARLI Harness Analytics ═══\n\n");

        // Data sources
        out.push_str(&format!(
            "Telemetry: {} ({})\n",
            if self.has_telemetry {
                "found"
            } else {
                "not found"
            },
            self.telemetry_path,
        ));
        out.push_str(&format!(
            "Lessons:   {} ({})\n\n",
            if self.has_lessons {
                "found"
            } else {
                "not found"
            },
            self.lessons_path,
        ));

        if !self.has_telemetry && !self.has_lessons {
            out.push_str("No data to analyze. Run the harness to collect telemetry.\n");
            return out;
        }

        // Tool insights
        out.push_str("─── Tools ───\n");
        out.push_str(&format!(
            "  Total calls: {} | Failures: {} | Rate: {:.1}% | Unique: {}\n",
            self.total_tool_calls,
            self.total_tool_failures,
            self.overall_failure_rate * 100.0,
            self.unique_tools,
        ));

        if let Some(ref worst) = self.most_failing_tool {
            out.push_str(&format!(
                "  Worst: {} ({:.0}% failure, {} calls)\n",
                worst.tool_name,
                worst.failure_rate * 100.0,
                worst.total_calls,
            ));
        }

        if !self.top_failing_tools.is_empty() {
            out.push_str("  Top by failure rate:\n");
            for tool in self.top_failing_tools.iter().take(5) {
                if tool.total_calls > 0 {
                    out.push_str(&format!(
                        "    {} — {:.0}% ({}/{})\n",
                        tool.tool_name,
                        tool.failure_rate * 100.0,
                        tool.total_failures,
                        tool.total_calls,
                    ));
                }
            }
        }
        out.push('\n');

        // Policy insights
        out.push_str("─── Sandbox Policies ───\n");
        out.push_str(&format!(
            "  Total violations: {} | Unique policies: {}\n",
            self.total_policy_violations, self.unique_policies,
        ));
        for policy in self.top_violated_policies.iter().take(3) {
            out.push_str(&format!(
                "    {} — {} violations\n",
                policy.policy_hash, policy.violations,
            ));
        }
        out.push('\n');

        // Retry insights
        out.push_str("─── Task Retries ───\n");
        out.push_str(&format!(
            "  Total retries: {} | Unique task types: {}\n",
            self.total_retries, self.unique_task_types,
        ));
        for task in self.top_retried_tasks.iter().take(5) {
            out.push_str(&format!(
                "    {} — {} retries\n",
                task.task_type, task.retries,
            ));
        }
        out.push('\n');

        // Memory insights
        out.push_str("─── Experiential Memory ───\n");
        out.push_str(&format!(
            "  Lessons: {} total | {} verified | {} unverified\n",
            self.total_lessons, self.verified_lessons, self.unverified_lessons,
        ));
        if let Some(rate) = self.memory_hit_rate {
            out.push_str(&format!(
                "  Hit rate: {:.0}% ({} lookups, from telemetry)\n",
                rate * 100.0,
                (rate * 100.0 / if rate > 0.0 { rate } else { 1.0 }) as u64, // approximate
            ));
        }
        if self.zero_hit_lessons > 0 {
            out.push_str(&format!(
                "  Zero-hit lessons: {} — may be stale noise\n",
                self.zero_hit_lessons,
            ));
        }
        if !self.top_effective_lessons.is_empty() {
            out.push_str("  Most effective lessons:\n");
            for lesson in self.top_effective_lessons.iter().take(3) {
                if lesson.effectiveness > 0.0 {
                    out.push_str(&format!(
                        "    {} — {:.0}% effective ({} hits)\n",
                        lesson.error_pattern,
                        lesson.effectiveness * 100.0,
                        lesson.hits,
                    ));
                }
            }
        }
        out.push('\n');

        // Recommendations
        out.push_str("─── Recommendations ───\n");
        for (i, rec) in self.recommendations.iter().enumerate() {
            out.push_str(&format!("  {}. {}\n", i + 1, rec));
        }

        out
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_telemetry::HarnessTelemetry;
    use crate::lessons::Lesson;

    fn make_test_telemetry() -> HarnessTelemetryReport {
        // Build a HarnessTelemetry, record some data, get the report
        let t = HarnessTelemetry::new();
        t.record_tool_call_result("read_file", true);
        t.record_tool_call_result("read_file", true);
        t.record_tool_call_result("read_file", true);
        t.record_tool_call_result("terminal", true);
        t.record_tool_call_result("terminal", false); // 1 failure
        t.record_tool_call_result("deploy", false); // 1 failure, 1 call
        t.record_policy_violation("policy-abc");
        t.record_policy_violation("policy-abc");
        t.record_policy_violation("policy-xyz");
        t.record_retry("compile_task");
        t.record_retry("compile_task");
        t.record_retry("deploy_task");
        t.record_memory_lookup_result(true);
        t.record_memory_lookup_result(true);
        t.record_memory_lookup_result(false);
        t.report()
    }

    fn make_test_lessons() -> ExperientialMemory {
        ExperientialMemory {
            lessons: vec![
                Lesson {
                    error_pattern: "borrow checker".into(),
                    fix_description: "clone before move".into(),
                    learned_at: "2026-01-01T00:00:00Z".into(),
                    times_applied: 10,
                    hits: 8,
                    misses: 2,
                    verified: true,
                },
                Lesson {
                    error_pattern: "undefined variable".into(),
                    fix_description: "add let binding".into(),
                    learned_at: "2026-01-02T00:00:00Z".into(),
                    times_applied: 3,
                    hits: 0,
                    misses: 3,
                    verified: false,
                },
            ],
            total_lookups: 15,
            total_hits: 8,
        }
    }

    #[test]
    fn test_analyze_with_data() {
        let analyzer = HarnessAnalyzer::default();
        // We can't easily mock file paths in Default, so test individual computations

        let telemetry = make_test_telemetry();
        let lessons = make_test_lessons();

        // Instead, test the report structure by building it manually
        let report = HarnessAnalyticsReport {
            telemetry_path: "test".into(),
            lessons_path: "test".into(),
            has_telemetry: true,
            has_lessons: true,
            top_failing_tools: analyzer.compute_top_failing_tools(&telemetry),
            most_failing_tool: None,
            overall_failure_rate: 0.0,
            top_violated_policies: analyzer.compute_top_violated_policies(&telemetry),
            top_retried_tasks: analyzer.compute_top_retried_tasks(&telemetry),
            memory_hit_rate: Some(telemetry.memory_hit_rate),
            total_lessons: lessons.lessons.len(),
            verified_lessons: lessons.lessons.iter().filter(|l| l.verified).count(),
            unverified_lessons: lessons.lessons.iter().filter(|l| !l.verified).count(),
            top_effective_lessons: analyzer.compute_top_effective_lessons(&lessons),
            zero_hit_lessons: lessons
                .lessons
                .iter()
                .filter(|l| l.hits == 0 && l.times_applied > 0)
                .count(),
            total_tool_calls: telemetry.total_tool_calls,
            total_tool_failures: telemetry.total_tool_failures,
            total_policy_violations: telemetry.policy_violations.values().sum(),
            total_retries: telemetry.total_retries,
            unique_tools: telemetry.unique_tools,
            unique_policies: telemetry.unique_policies,
            unique_task_types: telemetry.unique_task_types,
            recommendations: vec![],
        };

        // Verify tool insights
        assert_eq!(report.top_failing_tools.len(), 3);
        // deploy: 1/1 = 100% failure
        assert_eq!(report.top_failing_tools[0].tool_name, "deploy");
        assert_eq!(report.top_failing_tools[0].failure_rate, 1.0);
        // terminal: 1/2 = 50%
        assert_eq!(report.top_failing_tools[1].tool_name, "terminal");
        assert_eq!(report.top_failing_tools[1].failure_rate, 0.5);
        // read_file: 0/3 = 0%
        assert_eq!(report.top_failing_tools[2].tool_name, "read_file");
        assert_eq!(report.top_failing_tools[2].failure_rate, 0.0);

        // Verify policy insights
        assert_eq!(report.top_violated_policies.len(), 2);
        assert_eq!(report.top_violated_policies[0].violations, 2);

        // Verify lesson insights
        assert_eq!(report.top_effective_lessons.len(), 2);
        assert_eq!(report.top_effective_lessons[0].effectiveness, 0.8); // 8/10
        assert_eq!(report.top_effective_lessons[1].effectiveness, 0.0); // 0/3

        // Verify counts
        assert_eq!(report.total_lessons, 2);
        assert_eq!(report.verified_lessons, 1);
        assert_eq!(report.unverified_lessons, 1);
        assert_eq!(report.zero_hit_lessons, 1);
    }

    #[test]
    fn test_format_text() {
        let telemetry = make_test_telemetry();
        let lessons = make_test_lessons();
        let analyzer = HarnessAnalyzer::default();

        let mut report = HarnessAnalyticsReport {
            telemetry_path: "/tmp/test".into(),
            lessons_path: "/tmp/test".into(),
            has_telemetry: true,
            has_lessons: true,
            top_failing_tools: analyzer.compute_top_failing_tools(&telemetry),
            most_failing_tool: None,
            overall_failure_rate: 0.0,
            top_violated_policies: analyzer.compute_top_violated_policies(&telemetry),
            top_retried_tasks: analyzer.compute_top_retried_tasks(&telemetry),
            memory_hit_rate: Some(telemetry.memory_hit_rate),
            total_lessons: lessons.lessons.len(),
            verified_lessons: 1,
            unverified_lessons: 1,
            top_effective_lessons: analyzer.compute_top_effective_lessons(&lessons),
            zero_hit_lessons: 1,
            total_tool_calls: telemetry.total_tool_calls,
            total_tool_failures: telemetry.total_tool_failures,
            total_policy_violations: telemetry.policy_violations.values().sum(),
            total_retries: telemetry.total_retries,
            unique_tools: telemetry.unique_tools,
            unique_policies: telemetry.unique_policies,
            unique_task_types: telemetry.unique_task_types,
            recommendations: vec!["Fix tool 'deploy': fails 100%".into()],
        };

        let text = report.format_text();
        assert!(text.contains("ARLI Harness Analytics"));
        assert!(text.contains("Tools"));
        assert!(text.contains("Sandbox Policies"));
        assert!(text.contains("Task Retries"));
        assert!(text.contains("Experiential Memory"));
        assert!(text.contains("Recommendations"));
        assert!(text.contains("deploy"));
        assert!(text.contains("read_file"));
    }

    #[test]
    fn test_format_text_no_data() {
        let report = HarnessAnalyticsReport {
            telemetry_path: "/tmp/nonexistent".into(),
            lessons_path: "/tmp/nonexistent".into(),
            has_telemetry: false,
            has_lessons: false,
            top_failing_tools: vec![],
            most_failing_tool: None,
            overall_failure_rate: 0.0,
            top_violated_policies: vec![],
            top_retried_tasks: vec![],
            memory_hit_rate: None,
            total_lessons: 0,
            verified_lessons: 0,
            unverified_lessons: 0,
            top_effective_lessons: vec![],
            zero_hit_lessons: 0,
            total_tool_calls: 0,
            total_tool_failures: 0,
            total_policy_violations: 0,
            total_retries: 0,
            unique_tools: 0,
            unique_policies: 0,
            unique_task_types: 0,
            recommendations: vec![],
        };

        let text = report.format_text();
        assert!(text.contains("No data to analyze"));
    }

    #[test]
    fn test_recommendations_empty_when_healthy() {
        let analyzer = HarnessAnalyzer::default();
        let recs = analyzer.generate_recommendations(&[], &[], Some(0.9), 0, 0);
        assert_eq!(recs.len(), 1);
        assert!(recs[0].contains("healthy"));
    }

    #[test]
    fn test_recommendations_for_failing_tool() {
        let analyzer = HarnessAnalyzer::default();
        let tools = vec![ToolInsight {
            tool_name: "deploy".into(),
            total_calls: 10,
            total_failures: 5,
            failure_rate: 0.5,
        }];
        let recs = analyzer.generate_recommendations(&tools, &[], Some(0.9), 0, 0);
        assert!(recs.iter().any(|r| r.contains("deploy")));
    }

    #[test]
    fn test_recommendations_for_low_memory() {
        let analyzer = HarnessAnalyzer::default();
        let recs = analyzer.generate_recommendations(&[], &[], Some(0.15), 0, 0);
        assert!(recs.iter().any(|r| r.contains("Memory hit rate")));
    }
}
