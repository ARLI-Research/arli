//! Verification Pipeline — compile → lint → test → fuzz before attestation.
//!
//! From "Learning to generate unit tests for automated debugging" (COLM 2025)
//! and the ENCONTER verification loop: the harness must verify the agent's
//! work BEFORE submitting an attestation. The verification pipeline runs
//! compile, lint, test, and optionally fuzz steps in the agent's workspace,
//! and feeds results into the oracle's attestation decision.
//!
//! If any required step fails, the attestation is blocked — the contract
//! stays in Disputed until the agent fixes the failure.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

// ============================================================================
// VERIFICATION STEP
// ============================================================================

/// A single verification step in the pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerificationStep {
    /// Compile/lint check (e.g. `cargo check`, `cargo clippy`).
    Compile,
    /// Run tests (e.g. `cargo test`, `pytest`).
    Test,
    /// Lint/static analysis (e.g. `cargo clippy -- -D warnings`, `ruff check`).
    Lint,
    /// Fuzz testing (e.g. `cargo fuzz run`). Optional — only runs if fuzz targets exist.
    Fuzz,
}

impl VerificationStep {
    /// Human-readable name for this step.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Compile => "compile",
            Self::Test => "test",
            Self::Lint => "lint",
            Self::Fuzz => "fuzz",
        }
    }

    /// Is this step skippable on failure?
    ///
    /// Compile and Test are required — without them, attestation is blocked.
    /// Lint can be advisory (warnings don't block, only errors do).
    /// Fuzz is always optional.
    pub fn is_required(&self) -> bool {
        match self {
            Self::Compile | Self::Test => true,
            Self::Lint | Self::Fuzz => false,
        }
    }

    /// Get the commands for this step, given a workspace root.
    ///
    /// Auto-detects the build system based on marker files.
    pub fn commands_for_workspace(&self, workspace_root: &Path) -> Vec<String> {
        match self {
            Self::Compile => Self::detect_compile_commands(workspace_root),
            Self::Test => Self::detect_test_commands(workspace_root),
            Self::Lint => Self::detect_lint_commands(workspace_root),
            Self::Fuzz => Self::detect_fuzz_commands(workspace_root),
        }
    }

    fn detect_compile_commands(root: &Path) -> Vec<String> {
        if root.join("Cargo.toml").exists() {
            vec!["cargo check".into()]
        } else if root.join("package.json").exists() {
            vec!["npm run build".into()]
        } else if root.join("go.mod").exists() {
            vec!["go build ./...".into()]
        } else if root.join("Makefile").exists() {
            vec!["make".into()]
        } else {
            vec![]
        }
    }

    fn detect_test_commands(root: &Path) -> Vec<String> {
        if root.join("Cargo.toml").exists() {
            vec!["cargo test".into()]
        } else if root.join("package.json").exists() {
            vec!["npm test".into()]
        } else if root.join("pyproject.toml").exists() {
            vec!["pytest".into()]
        } else if root.join("setup.cfg").exists() {
            vec!["pytest".into()]
        } else if root.join("go.mod").exists() {
            vec!["go test ./...".into()]
        } else if root.join("Makefile").exists() {
            vec!["make test".into()]
        } else {
            vec![]
        }
    }

    fn detect_lint_commands(root: &Path) -> Vec<String> {
        if root.join("Cargo.toml").exists() {
            vec!["cargo clippy -- -D warnings".into()]
        } else if root.join("pyproject.toml").exists() {
            vec!["ruff check .".into()]
        } else if root.join("package.json").exists() {
            vec!["npm run lint".into()]
        } else if root.join("go.mod").exists() {
            vec!["go vet ./...".into()]
        } else {
            vec![]
        }
    }

    fn detect_fuzz_commands(root: &Path) -> Vec<String> {
        if root.join("fuzz").is_dir() && root.join("Cargo.toml").exists() {
            // Look for fuzz targets
            if let Ok(entries) = std::fs::read_dir(root.join("fuzz")) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() || path.extension().map_or(false, |e| e == "rs") {
                        let name = path.file_stem().unwrap_or_default().to_string_lossy();
                        return vec![format!("cargo fuzz run {}", name)];
                    }
                }
            }
        }
        vec![]
    }
}

// ============================================================================
// STEP RESULT
// ============================================================================

/// Result of a single verification step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Which step this result is for.
    pub step: VerificationStep,
    /// Whether the step passed (all commands exited 0).
    pub passed: bool,
    /// Combined stdout from all commands.
    pub stdout: String,
    /// Combined stderr from all commands.
    pub stderr: String,
    /// Exit code of the first failing command (0 if all passed).
    pub exit_code: i32,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// The actual commands that ran (after auto-detection).
    pub commands_executed: Vec<String>,
    /// Number of commands that ran.
    pub commands_count: usize,
    /// Number of commands that passed.
    pub commands_passed: usize,
}

impl StepResult {
    /// Create a result for a step that had no commands to run (e.g., no fuzz targets).
    pub fn skipped(step: VerificationStep) -> Self {
        Self {
            step,
            passed: true, // skipped ≠ failed
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            duration_ms: 0,
            commands_executed: vec![],
            commands_count: 0,
            commands_passed: 0,
        }
    }
}

// ============================================================================
// PIPELINE RESULT
// ============================================================================

/// Result of running the full verification pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineResult {
    /// Per-step results in execution order.
    pub steps: Vec<StepResult>,
    /// Whether ALL required steps passed.
    pub all_required_passed: bool,
    /// Whether ALL steps (including optional) passed.
    pub all_passed: bool,
    /// Total wall-clock duration in milliseconds.
    pub total_duration_ms: u64,
    /// Human-readable summary of what passed/failed.
    pub summary: String,
    /// Names of steps that failed (empty if all passed).
    pub failed_steps: Vec<String>,
}

impl PipelineResult {
    /// Create an empty result — no steps ran.
    pub fn empty() -> Self {
        Self {
            steps: vec![],
            all_required_passed: true, // vacuously true
            all_passed: true,
            total_duration_ms: 0,
            summary: "No verification steps configured".into(),
            failed_steps: vec![],
        }
    }

    /// Build the summary string from step results.
    pub fn build_summary(&mut self) {
        let parts: Vec<String> = self
            .steps
            .iter()
            .map(|s| {
                let icon = if s.passed { "✓" } else { "✗" };
                format!("{} {} ({}ms)", icon, s.step.name(), s.duration_ms)
            })
            .collect();
        self.summary = parts.join(" → ");
    }
}

// ============================================================================
// VERIFICATION PIPELINE
// ============================================================================

/// A verification pipeline: ordered list of steps to run before attestation.
#[derive(Debug, Clone)]
pub struct VerificationPipeline {
    /// Steps to execute in order.
    pub steps: Vec<VerificationStep>,
    /// Whether to stop on first failure (fail-fast).
    pub fail_fast: bool,
    /// Timeout per step in seconds (0 = no limit).
    pub timeout_per_step_secs: u64,
}

impl Default for VerificationPipeline {
    fn default() -> Self {
        Self {
            steps: vec![
                VerificationStep::Compile,
                VerificationStep::Lint,
                VerificationStep::Test,
                VerificationStep::Fuzz,
            ],
            fail_fast: true,
            timeout_per_step_secs: 300,
        }
    }
}

impl VerificationPipeline {
    /// Create a minimal pipeline — just compile + test (no lint, no fuzz).
    pub fn minimal() -> Self {
        Self {
            steps: vec![VerificationStep::Compile, VerificationStep::Test],
            fail_fast: true,
            timeout_per_step_secs: 300,
        }
    }

    /// Create a pipeline appropriate for the given sandbox profile.
    ///
    /// - Build: compile only (network disabled, can't run tests)
    /// - Test: compile + test + lint
    /// - Deploy: compile + lint + test + fuzz (full suite)
    /// - Unsafe: compile + test (minimal — trust the agent)
    pub fn for_sandbox_profile(profile: crate::sandbox_profile::SandboxProfile) -> Self {
        match profile {
            crate::sandbox_profile::SandboxProfile::Build => Self {
                steps: vec![VerificationStep::Compile],
                fail_fast: true,
                timeout_per_step_secs: 300,
            },
            crate::sandbox_profile::SandboxProfile::Test => Self {
                steps: vec![
                    VerificationStep::Compile,
                    VerificationStep::Lint,
                    VerificationStep::Test,
                ],
                fail_fast: true,
                timeout_per_step_secs: 600,
            },
            crate::sandbox_profile::SandboxProfile::Deploy => Self {
                steps: vec![
                    VerificationStep::Compile,
                    VerificationStep::Lint,
                    VerificationStep::Test,
                    VerificationStep::Fuzz,
                ],
                fail_fast: false, // don't block deploy on fuzz failure
                timeout_per_step_secs: 1200,
            },
            crate::sandbox_profile::SandboxProfile::Unsafe => Self::minimal(),
        }
    }

    /// Run the pipeline in the given workspace.
    ///
    /// Executes each step's commands in order. If `fail_fast` is true,
    /// stops at the first failure.
    pub fn run(&self, workspace_root: &Path) -> PipelineResult {
        let mut result = PipelineResult::empty();
        result.steps.clear();
        let start = Instant::now();

        for step in &self.steps {
            let step_result = self.execute_step(step, workspace_root);
            let step_passed = step_result.passed;

            result.steps.push(step_result);

            if self.fail_fast && !step_passed && step.is_required() {
                break;
            }
        }

        result.total_duration_ms = start.elapsed().as_millis() as u64;

        // Compute aggregate flags
        result.all_required_passed = result
            .steps
            .iter()
            .all(|s| s.passed || !s.step.is_required());

        result.all_passed = result.steps.iter().all(|s| s.passed);

        result.failed_steps = result
            .steps
            .iter()
            .filter(|s| !s.passed)
            .map(|s| s.step.name().to_string())
            .collect();

        result.build_summary();
        result
    }

    fn execute_step(&self, step: &VerificationStep, workspace_root: &Path) -> StepResult {
        let commands = step.commands_for_workspace(workspace_root);

        if commands.is_empty() {
            return StepResult::skipped(step.clone());
        }

        let step_start = Instant::now();
        let mut combined_stdout = String::new();
        let mut combined_stderr = String::new();
        let mut first_fail_code = 0;
        let mut passed_count = 0;

        for cmd in &commands {
            match self.run_command(cmd, workspace_root) {
                Ok((stdout, stderr, exit_code)) => {
                    if !stdout.is_empty() {
                        combined_stdout.push_str(&format!("--- {} ---\n{}\n", cmd, stdout));
                    }
                    if !stderr.is_empty() {
                        combined_stderr.push_str(&format!("--- {} ---\n{}\n", cmd, stderr));
                    }
                    if exit_code == 0 {
                        passed_count += 1;
                    } else if first_fail_code == 0 {
                        first_fail_code = exit_code;
                    }
                }
                Err(e) => {
                    combined_stderr.push_str(&format!("--- {} ---\nERROR: {}\n", cmd, e));
                    if first_fail_code == 0 {
                        first_fail_code = -1; // signal execution error
                    }
                }
            }
        }

        let duration_ms = step_start.elapsed().as_millis() as u64;

        StepResult {
            step: step.clone(),
            passed: first_fail_code == 0,
            stdout: combined_stdout,
            stderr: combined_stderr,
            exit_code: first_fail_code,
            duration_ms,
            commands_executed: commands.clone(),
            commands_count: commands.len(),
            commands_passed: passed_count,
        }
    }

    fn run_command(
        &self,
        cmd: &str,
        workspace_root: &Path,
    ) -> Result<(String, String, i32), String> {
        // Split command into program + args
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return Err("empty command".into());
        }

        let program = parts[0];
        let args = &parts[1..];

        let output = Command::new(program)
            .args(args)
            .current_dir(workspace_root)
            .output()
            .map_err(|e| format!("failed to execute '{}': {}", cmd, e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        Ok((stdout, stderr, exit_code))
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Step auto-detection ---

    #[test]
    fn test_detect_compile_rust() {
        let tmp = std::env::temp_dir().join("arli_test_vp_rust");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]").unwrap();

        let cmds = VerificationStep::Compile.commands_for_workspace(&tmp);
        assert_eq!(cmds, vec!["cargo check"]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_detect_test_go() {
        let tmp = std::env::temp_dir().join("arli_test_vp_go");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("go.mod"), "module test").unwrap();

        let cmds = VerificationStep::Test.commands_for_workspace(&tmp);
        assert_eq!(cmds, vec!["go test ./..."]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_detect_empty_workspace() {
        let tmp = std::env::temp_dir().join("arli_test_vp_empty");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let cmds = VerificationStep::Compile.commands_for_workspace(&tmp);
        assert!(cmds.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- Pipeline construction ---

    #[test]
    fn test_default_pipeline_has_all_steps() {
        let pipeline = VerificationPipeline::default();
        assert_eq!(pipeline.steps.len(), 4);
        assert_eq!(pipeline.steps[0], VerificationStep::Compile);
        assert_eq!(pipeline.steps[1], VerificationStep::Lint);
        assert_eq!(pipeline.steps[2], VerificationStep::Test);
        assert_eq!(pipeline.steps[3], VerificationStep::Fuzz);
    }

    #[test]
    fn test_minimal_pipeline() {
        let pipeline = VerificationPipeline::minimal();
        assert_eq!(pipeline.steps.len(), 2);
        assert_eq!(pipeline.steps[0], VerificationStep::Compile);
        assert_eq!(pipeline.steps[1], VerificationStep::Test);
    }

    // --- Pipeline execution ---

    #[test]
    fn test_pipeline_runs_in_empty_workspace() {
        let tmp = std::env::temp_dir().join("arli_test_vp_run_empty");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let pipeline = VerificationPipeline::minimal();
        let result = pipeline.run(&tmp);

        // All steps skipped (no commands detected) → all pass vacuously
        assert!(result.all_required_passed);
        assert!(result.all_passed);
        assert!(result.failed_steps.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_pipeline_summary_built() {
        let tmp = std::env::temp_dir().join("arli_test_vp_summary");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let pipeline = VerificationPipeline::minimal();
        let result = pipeline.run(&tmp);

        assert!(!result.summary.is_empty());
        // Should have step names with symbols
        assert!(result.summary.contains("compile"));
        assert!(result.summary.contains("test"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- StepResult::skipped ---

    #[test]
    fn test_skipped_step_passes() {
        let result = StepResult::skipped(VerificationStep::Fuzz);
        assert!(result.passed);
        assert_eq!(result.commands_count, 0);
        assert!(result.commands_executed.is_empty());
    }

    // --- is_required ---

    #[test]
    fn test_compile_is_required() {
        assert!(VerificationStep::Compile.is_required());
    }

    #[test]
    fn test_test_is_required() {
        assert!(VerificationStep::Test.is_required());
    }

    #[test]
    fn test_lint_is_not_required() {
        assert!(!VerificationStep::Lint.is_required());
    }

    #[test]
    fn test_fuzz_is_not_required() {
        assert!(!VerificationStep::Fuzz.is_required());
    }

    // --- fail_fast ---

    #[test]
    fn test_fail_fast_stops_on_compile_failure() {
        // Create a workspace with Cargo.toml but broken source that won't compile
        let tmp = std::env::temp_dir().join("arli_test_vp_failfast");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]\nname=\"broken\"").unwrap();

        let pipeline = VerificationPipeline {
            steps: vec![
                VerificationStep::Compile,
                VerificationStep::Test,
                VerificationStep::Lint,
            ],
            fail_fast: true,
            timeout_per_step_secs: 30,
        };

        let result = pipeline.run(&tmp);

        // Compile should fail (incomplete Cargo project)
        // fail_fast should stop before Test
        assert_eq!(result.steps.len(), 1); // only compile ran
        assert!(!result.all_required_passed);
        assert_eq!(result.failed_steps, vec!["compile"]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- Sandbox profile pipelines ---

    #[test]
    fn test_build_profile_pipeline() {
        let pipeline = VerificationPipeline::for_sandbox_profile(
            crate::sandbox_profile::SandboxProfile::Build,
        );
        assert_eq!(pipeline.steps.len(), 1);
        assert_eq!(pipeline.steps[0], VerificationStep::Compile);
        assert!(pipeline.fail_fast);
    }

    #[test]
    fn test_deploy_profile_pipeline_no_fail_fast() {
        let pipeline = VerificationPipeline::for_sandbox_profile(
            crate::sandbox_profile::SandboxProfile::Deploy,
        );
        assert_eq!(pipeline.steps.len(), 4);
        assert!(!pipeline.fail_fast); // deploy keeps going even if fuzz fails
    }

    // --- Serialization round-trip ---

    #[test]
    fn test_step_result_serialization() {
        let result = StepResult {
            step: VerificationStep::Compile,
            passed: true,
            stdout: "output".into(),
            stderr: "".into(),
            exit_code: 0,
            duration_ms: 1234,
            commands_executed: vec!["cargo check".into()],
            commands_count: 1,
            commands_passed: 1,
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: StepResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.step, VerificationStep::Compile);
        assert!(parsed.passed);
        assert_eq!(parsed.duration_ms, 1234);
    }

    #[test]
    fn test_pipeline_result_serialization() {
        let mut result = PipelineResult {
            steps: vec![
                StepResult::skipped(VerificationStep::Compile),
                StepResult::skipped(VerificationStep::Test),
            ],
            all_required_passed: true,
            all_passed: true,
            total_duration_ms: 0,
            summary: String::new(),
            failed_steps: vec![],
        };
        result.build_summary();

        let json = serde_json::to_string(&result).unwrap();
        let parsed: PipelineResult = serde_json::from_str(&json).unwrap();
        assert!(parsed.all_passed);
        assert_eq!(parsed.steps.len(), 2);
        assert!(!parsed.summary.is_empty());
    }
}
