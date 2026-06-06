//! Tool Output Compaction — trim verbose tool outputs before context injection.
//!
//! From "Code as Agent Harness" §3.2.6: long tool outputs (build logs, stack traces,
//! diff hunks) overload the context window. The harness should compact:
//! terminal → exit code + last N lines; test → failed names + key frames;
//! raw logs → truncate with link to full output.
//!
//! Full output is always saved to disk for audit and replay.

use std::path::PathBuf;

// ============================================================================
// COMPACTION CONFIG
// ============================================================================

/// Configuration for output compaction.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Maximum lines in compacted output (default: 50).
    pub max_lines: usize,
    /// Maximum characters in compacted output (default: 8000).
    pub max_chars: usize,
    /// Whether to save full output to disk (default: true).
    pub save_full_output: bool,
    /// Directory for full output files (default: ~/.arli/compacted/).
    pub output_dir: PathBuf,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Self {
            max_lines: 50,
            max_chars: 8000,
            save_full_output: true,
            output_dir: PathBuf::from(home).join(".arli").join("compacted"),
        }
    }
}

// ============================================================================
// COMPACTION RESULT
// ============================================================================

/// Result of compacting a tool output.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// Compact text to inject into context.
    pub compact_text: String,
    /// Path to full output (if saved).
    pub full_output_path: Option<PathBuf>,
    /// Original size in bytes.
    pub original_size: usize,
    /// Compacted size in bytes.
    pub compact_size: usize,
    /// Compression ratio (compact / original).
    pub compression_ratio: f64,
}

// ============================================================================
// TOOL OUTPUT COMPACTOR
// ============================================================================

/// Post-processes tool outputs to keep context compact.
pub struct ToolOutputCompactor {
    config: CompactionConfig,
}

impl ToolOutputCompactor {
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// Compact a tool output based on tool name hints.
    pub fn compact(
        &self,
        tool_name: &str,
        output: &str,
        exit_code: Option<i32>,
    ) -> CompactionResult {
        let original_size = output.len();

        // If output is already small, don't compact
        if output.len() <= self.config.max_chars && output.lines().count() <= self.config.max_lines {
            return CompactionResult {
                compact_text: output.to_string(),
                full_output_path: None,
                original_size,
                compact_size: output.len(),
                compression_ratio: 1.0,
            };
        }

        // Save full output to disk
        let full_path = if self.config.save_full_output {
            self.save_full_output(tool_name, output).ok()
        } else {
            None
        };

        // Compact based on tool type
        let compact_text = match tool_name {
            "terminal" | "bash" | "execute_code" => self.compact_terminal(output, exit_code),
            "test" | "cargo test" | "pytest" => self.compact_test(output),
            "cargo build" | "build" => self.compact_build(output),
            _ => self.compact_generic(output),
        };

        // Apply character limit as final safety net
        let compact_text = self.truncate_chars(&compact_text);

        // Append pointer to full output
        let compact_text = if let Some(ref path) = full_path {
            format!(
                "{}\n\n[Full output ({:.1}KB) saved to {}]",
                compact_text,
                original_size as f64 / 1024.0,
                path.display()
            )
        } else {
            compact_text
        };

        let compact_size = compact_text.len();
        let compression_ratio = if original_size > 0 {
            compact_size as f64 / original_size as f64
        } else {
            1.0
        };

        CompactionResult {
            compact_text,
            full_output_path: full_path,
            original_size,
            compact_size,
            compression_ratio,
        }
    }

    // --- Tool-specific compaction strategies ---

    /// Terminal output: keep exit code + first error lines + last N lines.
    fn compact_terminal(&self, output: &str, exit_code: Option<i32>) -> String {
        let lines: Vec<&str> = output.lines().collect();
        let total = lines.len();

        if total <= self.config.max_lines {
            return output.to_string();
        }

        let mut result = String::new();

        // Header: exit code
        if let Some(code) = exit_code {
            let status = if code == 0 { "OK" } else { "FAILED" };
            result.push_str(&format!("Exit code: {} ({})\n", code, status));
        }

        // Find error lines
        let error_indices: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                let lower = line.to_lowercase();
                lower.contains("error")
                    || lower.contains("fail")
                    || lower.contains("traceback")
                    || lower.contains("panic")
                    || lower.contains("exception")
            })
            .map(|(i, _)| i)
            .collect();

        // Extract error blocks (context around error lines)
        if !error_indices.is_empty() {
            result.push_str("--- Errors ---\n");
            let mut shown = std::collections::HashSet::new();
            for &ei in &error_indices {
                if shown.contains(&ei) {
                    continue;
                }
                let start = ei.saturating_sub(1);
                let end = (ei + 3).min(total);
                for i in start..end {
                    result.push_str(lines[i]);
                    result.push('\n');
                    shown.insert(i);
                }
                result.push_str("---\n");
                if result.lines().count() > self.config.max_lines / 2 {
                    break;
                }
            }
        }

        // Tail: last 20 lines
        let tail_start = total.saturating_sub(20);
        if tail_start > 0 {
            result.push_str(&format!("\n--- Last 20 lines (of {}) ---\n", total));
            for line in &lines[tail_start..] {
                result.push_str(line);
                result.push('\n');
            }
        }

        result
    }

    /// Test output: keep summary line + failed test names.
    fn compact_test(&self, output: &str) -> String {
        let lines: Vec<&str> = output.lines().collect();
        let mut result = String::new();

        for line in &lines {
            let lower = line.to_lowercase();
            if lower.contains("test result")
                || lower.contains("passed")
                || lower.contains("failures")
            {
                result.push_str(line);
                result.push('\n');
            }
        }

        for line in &lines {
            if line.trim_start().starts_with("test ")
                && (line.contains("FAILED") || line.contains("... FAILED"))
            {
                result.push_str(line);
                result.push('\n');
            }
        }

        if result.trim().is_empty() {
            return self.compact_generic(output);
        }

        result
    }

    /// Build output: keep final status + error lines + warnings.
    fn compact_build(&self, output: &str) -> String {
        let lines: Vec<&str> = output.lines().collect();
        let mut result = String::new();

        for line in &lines {
            let lower = line.to_lowercase();
            if lower.contains("error") || lower.contains("warning") {
                result.push_str(line);
                result.push('\n');
            }
        }

        if let Some(last) = lines.last() {
            if !last.is_empty() {
                result.push_str(last);
                result.push('\n');
            }
        }

        if result.trim().is_empty() {
            self.compact_generic(output)
        } else {
            result
        }
    }

    /// Generic: keep first 5 + last 40 lines.
    fn compact_generic(&self, output: &str) -> String {
        let lines: Vec<&str> = output.lines().collect();
        let total = lines.len();

        if total <= self.config.max_lines {
            return output.to_string();
        }

        let head_count = 5.min(total);
        let tail_count = self.config.max_lines.saturating_sub(head_count);
        let tail_start = total.saturating_sub(tail_count);

        let mut result = String::new();
        for line in &lines[..head_count] {
            result.push_str(line);
            result.push('\n');
        }
        if tail_start > head_count {
            result.push_str(&format!(
                "... [{} lines skipped] ...\n",
                tail_start - head_count
            ));
        }
        for line in &lines[tail_start..] {
            result.push_str(line);
            result.push('\n');
        }
        result
    }

    // --- Helpers ---

    fn truncate_chars(&self, text: &str) -> String {
        if text.len() <= self.config.max_chars {
            return text.to_string();
        }
        let mut truncated = text[..self.config.max_chars].to_string();
        truncated.push_str(&format!(
            "\n... [truncated at {} chars, {} total]",
            self.config.max_chars,
            text.len()
        ));
        truncated
    }

    fn save_full_output(&self, tool_name: &str, output: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.config.output_dir)
            .map_err(|e| format!("create output dir: {}", e))?;

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let safe_name = tool_name.replace(['/', ' ', '\\'], "_");
        let filename = format!("{}_{}.log", safe_name, timestamp);
        let path = self.config.output_dir.join(&filename);

        std::fs::write(&path, output).map_err(|e| format!("write output: {}", e))?;
        Ok(path)
    }
}

impl Default for ToolOutputCompactor {
    fn default() -> Self {
        Self::new(CompactionConfig::default())
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CompactionConfig {
        CompactionConfig {
            max_lines: 50,
            max_chars: 8000,
            save_full_output: true,
            output_dir: std::env::temp_dir().join("arli_test_compacted"),
        }
    }

    #[test]
    fn test_small_output_passes_through() {
        let c = ToolOutputCompactor::new(test_config());
        let output = "Hello world";
        let result = c.compact("terminal", output, Some(0));
        assert_eq!(result.compact_text, output);
        assert_eq!(result.compression_ratio, 1.0);
    }

    #[test]
    fn test_terminal_compaction_keeps_errors() {
        let c = ToolOutputCompactor::new(test_config());
        let output = "line1\nline2\nline3\n".repeat(100)
            + "error: something went wrong\n"
            + &"lineX\n".repeat(50);
        let result = c.compact("terminal", &output, Some(1));

        assert!(result.compact_text.contains("Exit code: 1"));
        assert!(result.compact_text.contains("error: something went wrong"));
        assert!(result.compact_text.contains("Last 20 lines"));
        assert!(result.compression_ratio < 1.0);
    }

    #[test]
    fn test_test_output_extracts_failures() {
        let c = ToolOutputCompactor::new(test_config());
        let output = "running 10 tests\ntest foo ... ok\ntest bar ... FAILED\ntest baz ... ok\ntest result: FAILED. 1 passed; 1 failed;";
        let result = c.compact("test", output, None);

        assert!(result.compact_text.contains("test bar ... FAILED"));
        assert!(result.compact_text.contains("test result"));
    }

    #[test]
    fn test_build_output_keeps_errors_and_warnings() {
        let c = ToolOutputCompactor::new(test_config());
        let output = "Compiling foo\nwarning: unused variable\n   Compiling bar\nerror[E0001]: compilation failed\n   --> src/main.rs:1:1";
        let result = c.compact("cargo build", output, None);

        assert!(result.compact_text.contains("warning: unused variable"));
        assert!(result
            .compact_text
            .contains("error[E0001]: compilation failed"));
    }

    #[test]
    fn test_generic_truncation() {
        let c = ToolOutputCompactor::new(test_config());
        let output = "line\n".repeat(200);
        let result = c.compact("search", &output, None);

        assert!(result.compact_text.contains("lines skipped"));
        assert!(result.compact_text.lines().count() <= 55);
    }

    #[test]
    fn test_full_output_saved() {
        let c = ToolOutputCompactor::new(test_config());
        let output = "important log data\n".repeat(100);
        let result = c.compact("terminal", &output, Some(0));

        assert!(result.full_output_path.is_some());
        assert!(result.full_output_path.unwrap().exists());
    }

    #[test]
    fn test_char_limit_enforced() {
        let mut config = test_config();
        config.max_chars = 100;
        let c = ToolOutputCompactor::new(config);
        let output = "A".repeat(5000);
        let result = c.compact("generic", &output, None);

        // With max_chars=100, the 5000-char output should be compacted
        assert!(result.compact_text.len() <= 500);
        assert!(result.compact_text.contains("truncated"));
    }

    #[test]
    fn test_zero_lines_output() {
        let c = ToolOutputCompactor::new(test_config());
        let result = c.compact("terminal", "", Some(0));
        assert_eq!(result.compact_text, "");
        assert_eq!(result.compression_ratio, 1.0);
    }
}
