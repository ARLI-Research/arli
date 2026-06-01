//! Delegate task tool — spawn child agents from within an agent.
//!
//! This is how agents autonomously parallelize work: parent spawns children,
//! children execute independently, parent receives results.
//!
//! Two modes:
//! - spawn: fire-and-forget, returns child_id immediately
//! - spawn_and_wait: spawns and polls until complete, returns result

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

use super::{Tool, ToolOutput};
use crate::swarm::{Swarm, SwarmAgentConfig, SwarmEvent};

/// Tool for spawning child agents.
pub struct DelegateTaskTool {
    swarm: Arc<Swarm>,
}

impl DelegateTaskTool {
    pub fn new(swarm: Arc<Swarm>) -> Self {
        Self { swarm }
    }
}

#[async_trait]
impl Tool for DelegateTaskTool {
    fn name(&self) -> &str {
        "delegate_task"
    }

    fn description(&self) -> &str {
        "Spawn a child agent to work on a subtask independently. \
         Use this to parallelize work or offload complex subtasks. \
         Modes: 'spawn' (returns child_id immediately), \
         'spawn_and_wait' (blocks until child completes, up to timeout_secs). \
         Also: 'list' (show all children), 'kill' (stop a child), \
         'status' (check child status)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["spawn", "spawn_and_wait", "list", "kill", "status"],
                    "description": "What to do: spawn, spawn_and_wait, list children, kill a child, or check status"
                },
                "goal": {
                    "type": "string",
                    "description": "What the child agent should accomplish (for spawn/spawn_and_wait)"
                },
                "context": {
                    "type": "string",
                    "description": "Background info the child needs: file paths, error messages, constraints"
                },
                "child_id": {
                    "type": "string",
                    "description": "Child agent ID (for kill/status)"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Max seconds to wait for child completion (for spawn_and_wait)",
                    "default": 120
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
            "spawn" => self.handle_spawn(&args, false).await,
            "spawn_and_wait" => self.handle_spawn(&args, true).await,
            "list" => self.handle_list().await,
            "kill" => self.handle_kill(&args).await,
            "status" => self.handle_status(&args).await,
            other => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!(
                    "Unknown action: {}. Valid: spawn, spawn_and_wait, list, kill, status",
                    other
                )),
            },
        }
    }
}

impl DelegateTaskTool {
    async fn handle_spawn(&self, args: &serde_json::Value, wait: bool) -> ToolOutput {
        let goal = match args["goal"].as_str() {
            Some(g) => g.to_string(),
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'goal' parameter".into()),
                }
            }
        };

        let context = args["context"].as_str().unwrap_or("");
        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(120);

        // Build the initial message
        let initial_message = if context.is_empty() {
            goal.clone()
        } else {
            format!("{}\n\nContext:\n{}", goal, context)
        };

        let config = SwarmAgentConfig {
            name: format!("child-{}", &goal[..goal.len().min(30)]),
            system_prompt: Some(
                "You are a focused sub-agent. Complete your assigned task \
                 efficiently and return only the result. Do not ask questions \
                 — use tools to gather information and produce your answer. \
                 When done, state your final result clearly."
                    .to_string(),
            ),
            initial_message: Some(initial_message),
            max_iterations: 10,
            restart_policy: None,
            parent_id: None,
        };

        match self.swarm.spawn(config).await {
            Ok(child_id) => {
                if !wait {
                    ToolOutput {
                        success: true,
                        content: format!(
                            "Child agent spawned: {}\n\
                             Goal: {}\n\
                             Use delegate_task action='status' child_id='{}' to check progress.\n\
                             Use delegate_task action='kill' child_id='{}' to stop it.",
                            child_id, goal, child_id, child_id
                        ),
                        error: None,
                    }
                } else {
                    // Wait for completion
                    self.wait_for_child(&child_id, timeout_secs).await
                }
            }
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Failed to spawn child: {}", e)),
            },
        }
    }

    async fn wait_for_child(&self, child_id: &str, timeout_secs: u64) -> ToolOutput {
        let mut events = self.swarm.subscribe();
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                // Timeout — kill the child
                if let Some(handle) = self.swarm.get(&child_id.to_string()).await {
                    handle.kill().await.ok();
                }
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!(
                        "Child {} timed out after {}s and was killed",
                        child_id, timeout_secs
                    )),
                };
            }

            match tokio::time::timeout(remaining, events.recv()).await {
                Ok(Ok(SwarmEvent::AgentCompleted { id, result })) if id == child_id => {
                    return ToolOutput {
                        success: true,
                        content: format!("Child {} completed:\n{}", child_id, result),
                        error: None,
                    };
                }
                Ok(Ok(SwarmEvent::AgentFailed { id, error })) if id == child_id => {
                    return ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some(format!("Child {} failed: {}", child_id, error)),
                    };
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some("Event channel closed".into()),
                    };
                }
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    tracing::warn!("Event receiver lagged by {} messages", n);
                    continue;
                }
                Err(_) => {
                    // Timeout expired — handled in next loop iteration
                    continue;
                }
                Ok(_) => {
                    // Event for a different child — ignore
                    continue;
                }
            }
        }
    }

    async fn handle_list(&self) -> ToolOutput {
        let agents = self.swarm.list().await;
        if agents.is_empty() {
            return ToolOutput {
                success: true,
                content: "No child agents running.".into(),
                error: None,
            };
        }

        let mut out = String::from("Child agents:\n");
        for (id, name, status) in &agents {
            out.push_str(&format!("  {} | {} | {:?}\n", id, name, status));
        }

        ToolOutput {
            success: true,
            content: out,
            error: None,
        }
    }

    async fn handle_kill(&self, args: &serde_json::Value) -> ToolOutput {
        let child_id = match args["child_id"].as_str() {
            Some(id) => id,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'child_id' parameter".into()),
                }
            }
        };

        match self.swarm.get(&child_id.to_string()).await {
            Some(handle) => match handle.kill().await {
                Ok(()) => ToolOutput {
                    success: true,
                    content: format!("Child {} killed.", child_id),
                    error: None,
                },
                Err(e) => ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Failed to kill child {}: {}", child_id, e)),
                },
            },
            None => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Child {} not found", child_id)),
            },
        }
    }

    async fn handle_status(&self, args: &serde_json::Value) -> ToolOutput {
        let child_id = match args["child_id"].as_str() {
            Some(id) => id,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'child_id' parameter".into()),
                }
            }
        };

        match self.swarm.status(&child_id.to_string()).await {
            Some(status) => ToolOutput {
                success: true,
                content: format!("Child {} status: {:?}", child_id, status),
                error: None,
            },
            None => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Child {} not found", child_id)),
            },
        }
    }
}
