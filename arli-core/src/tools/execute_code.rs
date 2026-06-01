//! execute_code — sandboxed Python code execution.
//!
//! Provides the agent with the ability to run Python scripts
//! in an isolated environment. Uses Linux namespaces when available,
//! falls back to direct process execution otherwise.
//!
//! Similar to Hermes' execute_code and Claude Code's sandbox.

use async_trait::async_trait;
use std::time::Instant;

use super::{Tool, ToolOutput};
use crate::sandbox::{Sandbox, SandboxConfig};

pub struct ExecuteCodeTool;

#[async_trait]
impl Tool for ExecuteCodeTool {
    fn name(&self) -> &str {
        "execute_code"
    }

    fn description(&self) -> &str {
        "Execute a Python script in a sandboxed environment. \
         Use for data processing, API calls, file operations, \
         or any task requiring programmatic logic. \
         The script receives full Python stdlib. \
         Output is capped at ~50KB. Timeout: 60s. \
         Print results to stdout — they will be returned to you."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Python code to execute"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Max execution time in seconds (default: 60)",
                    "default": 60
                }
            },
            "required": ["code"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Invalid JSON: {}", e)),
                }
            }
        };

        let code = match args["code"].as_str() {
            Some(c) => c,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: code".into()),
                }
            }
        };

        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(60);

        let start = Instant::now();
        let result = Self::run_python(code, timeout_secs);
        let elapsed_ms = start.elapsed().as_millis();

        match result {
            Ok(mut stdout) => {
                // Cap output at 50KB
                let max_len = 50 * 1024;
                if stdout.len() > max_len {
                    stdout.truncate(max_len);
                    stdout.push_str("\n\n[Output truncated at 50KB]");
                }

                let header = format!("Code executed in {}ms. Output:\n\n", elapsed_ms);

                ToolOutput {
                    success: true,
                    content: format!("{}{}", header, stdout),
                    error: None,
                }
            }
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(e),
            },
        }
    }
}

impl ExecuteCodeTool {
    /// Run Python code through the sandbox.
    fn run_python(code: &str, timeout_secs: u64) -> Result<String, String> {
        // Escape the code for safe shell embedding
        let escaped = code.replace('\\', "\\\\").replace('\'', "'\\''");

        let command = format!("python3 -c '{}'", escaped);

        let config = SandboxConfig {
            timeout_secs,
            ..SandboxConfig::default()
        };

        let output = Sandbox::execute(&command, &config);

        if !output.stderr.is_empty() && !output.success {
            return Err(format!(
                "Python error (exit code {}):\n{}",
                output.exit_code, output.stderr
            ));
        }

        if !output.success {
            return Err(format!(
                "Sandbox execution failed (exit code {}).\nstdout: {}\nstderr: {}",
                output.exit_code, output.stdout, output.stderr
            ));
        }

        if output.killed_by_timeout {
            return Err(format!(
                "Execution timed out after {}s. Partial output:\n{}",
                timeout_secs, output.stdout
            ));
        }

        let result = if output.stdout.is_empty() && !output.stderr.is_empty() {
            // Python often prints to stderr — include it
            output.stderr
        } else if !output.stdout.is_empty() {
            output.stdout
        } else {
            String::from("(no output)")
        };

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_simple_print() {
        let tool = ExecuteCodeTool;
        let result = tool
            .execute(&serde_json::json!({"code": "print('hello from sandbox')"}).to_string())
            .await;

        assert!(result.success, "Error: {:?}", result.error);
        assert!(result.content.contains("hello from sandbox"));
    }

    #[tokio::test]
    async fn test_execute_arithmetic() {
        let tool = ExecuteCodeTool;
        let result = tool
            .execute(&serde_json::json!({"code": "print(2 + 2)"}).to_string())
            .await;

        assert!(result.success);
        assert!(result.content.contains('4'));
    }

    #[tokio::test]
    async fn test_execute_syntax_error() {
        let tool = ExecuteCodeTool;
        let result = tool
            .execute(&serde_json::json!({"code": "print("}).to_string())
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("SyntaxError"));
    }

    #[tokio::test]
    async fn test_execute_multiline() {
        let tool = ExecuteCodeTool;
        let result = tool
            .execute(
                &serde_json::json!({
                    "code": "import json\ndata = {'key': 'value'}\nprint(json.dumps(data))"
                })
                .to_string(),
            )
            .await;

        assert!(result.success);
        assert!(result.content.contains(r#""key""#));
        assert!(result.content.contains(r#""value""#));
    }
}
