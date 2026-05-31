use async_trait::async_trait;
use serde_json;

use super::{Tool, ToolOutput};

/// read_file — reads a file from disk with offset/limit pagination.
///
/// Mirrors the pattern used by every major harness (Hermes, Claude Code, Pi).
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a file from disk. Use offset and limit for large files. \
         Lines are 1-indexed. Defaults to reading the first 500 lines."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-indexed)",
                    "default": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read",
                    "default": 500
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Invalid JSON arguments: {}", e)),
                }
            }
        };

        let path = match args["path"].as_str() {
            Some(p) => p,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: path".into()),
                }
            }
        };

        let offset = args["offset"].as_u64().unwrap_or(1) as usize;
        let limit = args["limit"].as_u64().unwrap_or(500) as usize;

        // Read the file
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();

                if offset > total_lines {
                    return ToolOutput {
                        success: true,
                        content: format!(
                            "File has {} lines total. Requested offset {} is beyond end.\n\
                             Use offset=1 to read from the beginning.",
                            total_lines, offset
                        ),
                        error: None,
                    };
                }

                let start = offset.saturating_sub(1);
                let end = (start + limit).min(total_lines);
                let selected: Vec<String> = lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:6}|{}", start + i + 1, line))
                    .collect();

                let mut output = selected.join("\n");

                if end < total_lines {
                    output.push_str(&format!(
                        "\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                        start + 1,
                        end,
                        total_lines,
                        end + 1
                    ));
                }

                ToolOutput {
                    success: true,
                    content: output,
                    error: None,
                }
            }
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Cannot read '{}': {}", path, e)),
            },
        }
    }
}

/// write_file — creates or overwrites a file.
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating parent directories if needed. \
         Overwrites existing files completely."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Invalid JSON arguments: {}", e)),
                }
            }
        };

        let path = match args["path"].as_str() {
            Some(p) => p,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: path".into()),
                }
            }
        };

        let content = match args["content"].as_str() {
            Some(c) => c,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: content".into()),
                }
            }
        };

        // Create parent directories
        if let Some(parent) = std::path::Path::new(path).parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Cannot create directory: {}", e)),
                };
            }
        }

        match std::fs::write(path, content) {
            Ok(_) => {
                let size = content.len();
                ToolOutput {
                    success: true,
                    content: format!("Wrote {} bytes to '{}'", size, path),
                    error: None,
                }
            }
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Cannot write '{}': {}", path, e)),
            },
        }
    }
}

/// shell — executes a shell command and returns output.
pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return stdout, stderr, and exit code. \
         Use for builds, git, package managers, and system operations."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "workdir": {
                    "type": "string",
                    "description": "Working directory for the command"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Invalid JSON arguments: {}", e)),
                }
            }
        };

        let command = match args["command"].as_str() {
            Some(c) => c,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: command".into()),
                }
            }
        };

        let workdir = args["workdir"].as_str();

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(command);

        if let Some(dir) = workdir {
            cmd.current_dir(dir);
        }

        match cmd.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str("--- stderr ---\n");
                    result.push_str(&stderr);
                }

                ToolOutput {
                    success: exit_code == 0,
                    content: result,
                    error: if exit_code != 0 {
                        Some(format!("Exit code: {}", exit_code))
                    } else {
                        None
                    },
                }
            }
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Cannot execute '{}': {}", command, e)),
            },
        }
    }
}

use std::path::PathBuf;
use std::sync::Arc;
use crate::memory::MemoryStore;
use crate::process::ProcessManager;
use crate::swarm::Swarm;

use super::search::SessionSearchTool;
use super::search_files::SearchFilesTool;
use super::http_get::HttpGetTool;
use super::web_search::WebSearchTool;
use super::vision::VisionTool;
use super::voice::VoiceTool;
use super::memory::MemoryTool;
use super::delegate::DelegateTaskTool;
use super::execute_code::ExecuteCodeTool;
use super::patch::PatchTool;
use super::process::ProcessTool;
use super::browser::BrowserTool;

/// Register all built-in tools.
/// Pass a database path to enable the session_search tool.
/// Pass a memory store to enable the memory tool.
/// Pass a swarm to enable the delegate_task tool.
/// Pass a process manager to enable the process tool.
pub fn register_builtin_tools(
    registry: &mut super::ToolRegistry,
    db_path: Option<PathBuf>,
    memory_store: Option<Arc<MemoryStore>>,
    swarm: Option<Arc<Swarm>>,
    process_manager: Option<Arc<ProcessManager>>,
) {
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(ShellTool));
    registry.register(Box::new(SearchFilesTool));
    registry.register(Box::new(HttpGetTool));
    registry.register(Box::new(WebSearchTool));
    registry.register(Box::new(VisionTool));
    registry.register(Box::new(VoiceTool));
    registry.register(Box::new(PatchTool));
    registry.register(Box::new(BrowserTool));
    registry.register(Box::new(ExecuteCodeTool));

    if let Some(path) = db_path {
        registry.register(Box::new(SessionSearchTool::new(path)));
    }

    if let Some(store) = memory_store {
        registry.register(Box::new(MemoryTool::new(store)));
    }

    if let Some(swarm) = swarm {
        registry.register(Box::new(DelegateTaskTool::new(swarm)));
    }

    if let Some(pm) = process_manager {
        registry.register(Box::new(ProcessTool::new(pm)));
    }
}
