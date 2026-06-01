//! Process tool — manage background processes from within the agent.
//!
//! Actions: spawn, poll, wait, kill, list, remove.
//! Wraps crate::process::ProcessManager.

use async_trait::async_trait;
use std::sync::Arc;

use super::{Tool, ToolOutput};
use crate::process::ProcessManager;

pub struct ProcessTool {
    manager: Arc<ProcessManager>,
}

impl ProcessTool {
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for ProcessTool {
    fn name(&self) -> &str {
        "process"
    }

    fn description(&self) -> &str {
        "Manage background processes. Actions: \
         'spawn' (start a command in background, returns id), \
         'poll' (check status + new output), \
         'wait' (block until completion or timeout), \
         'kill' (terminate by id), \
         'list' (show all processes), \
         'remove' (clean up completed process). \
         Use this for long-running commands: builds, servers, batch jobs."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["spawn", "poll", "wait", "kill", "list", "remove"],
                    "description": "What to do with the process"
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to run (for 'spawn')"
                },
                "workdir": {
                    "type": "string",
                    "description": "Working directory for the command (for 'spawn')"
                },
                "session_id": {
                    "type": "string",
                    "description": "Process ID (for 'poll', 'wait', 'kill', 'remove')"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Max seconds to wait (for 'wait')",
                    "default": 60
                }
            },
            "required": ["action"]
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

        let action = match args["action"].as_str() {
            Some(a) => a,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'action' parameter".into()),
                }
            }
        };

        match action {
            "spawn" => self.handle_spawn(&args),
            "poll" => self.handle_poll(&args),
            "wait" => self.handle_wait(&args),
            "kill" => self.handle_kill(&args),
            "list" => self.handle_list(),
            "remove" => self.handle_remove(&args),
            other => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!(
                    "Unknown action: {}. Valid: spawn, poll, wait, kill, list, remove",
                    other
                )),
            },
        }
    }
}

impl ProcessTool {
    fn handle_spawn(&self, args: &serde_json::Value) -> ToolOutput {
        let command = match args["command"].as_str() {
            Some(c) => c,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'command' for spawn".into()),
                }
            }
        };

        let workdir = args["workdir"].as_str();

        match self.manager.spawn(command, workdir) {
            Ok(id) => ToolOutput {
                success: true,
                content: format!(
                    "Process spawned: {}\nCommand: {}\nUse process action='poll' session_id='{}' to check.",
                    id, command, id
                ),
                error: None,
            },
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Spawn failed: {}", e)),
            },
        }
    }

    fn handle_poll(&self, args: &serde_json::Value) -> ToolOutput {
        let id = match args["session_id"].as_str() {
            Some(id) => id,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'session_id' for poll".into()),
                }
            }
        };

        match self.manager.poll(id) {
            Ok(result) => {
                let mut out = format!(
                    "Process {} — status: {:?} — elapsed: {}s\n",
                    id, result.status, result.elapsed_secs
                );

                if !result.new_stdout.is_empty() {
                    out.push_str("\n--- stdout ---\n");
                    out.push_str(&result.new_stdout.join("\n"));
                }

                if !result.new_stderr.is_empty() {
                    out.push_str("\n--- stderr ---\n");
                    out.push_str(&result.new_stderr.join("\n"));
                }

                ToolOutput {
                    success: true,
                    content: out,
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

    fn handle_wait(&self, args: &serde_json::Value) -> ToolOutput {
        let id = match args["session_id"].as_str() {
            Some(id) => id,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'session_id' for wait".into()),
                }
            }
        };

        let timeout_secs = args["timeout_secs"].as_u64();

        match self.manager.wait(id, timeout_secs) {
            Ok(result) => {
                if result.timed_out {
                    ToolOutput {
                        success: false,
                        content: format!(
                            "Process {} timed out after {}s.\nPartial output:\n{}",
                            id, result.elapsed_secs, result.stdout
                        ),
                        error: Some("Timeout".into()),
                    }
                } else if result.exit_code == 0 {
                    ToolOutput {
                        success: true,
                        content: format!(
                            "Process {} completed in {}s (exit 0):\n{}",
                            id, result.elapsed_secs, result.stdout
                        ),
                        error: None,
                    }
                } else {
                    ToolOutput {
                        success: false,
                        content: format!(
                            "Process {} failed in {}s (exit {}):\nstdout:\n{}\nstderr:\n{}",
                            id, result.elapsed_secs, result.exit_code, result.stdout, result.stderr
                        ),
                        error: Some(format!("Exit code: {}", result.exit_code)),
                    }
                }
            }
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(e),
            },
        }
    }

    fn handle_kill(&self, args: &serde_json::Value) -> ToolOutput {
        let id = match args["session_id"].as_str() {
            Some(id) => id,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'session_id' for kill".into()),
                }
            }
        };

        match self.manager.kill(id) {
            Ok(()) => ToolOutput {
                success: true,
                content: format!("Process {} killed.", id),
                error: None,
            },
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(e),
            },
        }
    }

    fn handle_list(&self) -> ToolOutput {
        let processes = self.manager.list();

        if processes.is_empty() {
            return ToolOutput {
                success: true,
                content: "No background processes.".into(),
                error: None,
            };
        }

        let mut out = format!("{} background process(es):\n", processes.len());
        for p in &processes {
            out.push_str(&format!(
                "  {} | {:?} | {}s | {}\n",
                &p.id[..p.id.len().min(12)],
                p.status,
                p.elapsed_secs,
                p.command
            ));
        }

        ToolOutput {
            success: true,
            content: out,
            error: None,
        }
    }

    fn handle_remove(&self, args: &serde_json::Value) -> ToolOutput {
        let id = match args["session_id"].as_str() {
            Some(id) => id,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'session_id' for remove".into()),
                }
            }
        };

        match self.manager.remove(id) {
            Ok(()) => ToolOutput {
                success: true,
                content: format!("Process {} removed from manager.", id),
                error: None,
            },
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessStatus;

    #[test]
    fn test_process_spawn_and_list() {
        let pm = Arc::new(ProcessManager::new());

        // Spawn a quick command
        let id = pm.spawn("echo test123", None).unwrap();
        assert!(!id.is_empty());

        // List should show the process
        let list = pm.list();
        assert!(!list.is_empty());

        // Wait for it to complete
        std::thread::sleep(std::time::Duration::from_millis(500));
        let poll = pm.poll(&id).unwrap();
        assert!(matches!(poll.status, ProcessStatus::Completed { .. }));
    }
}
