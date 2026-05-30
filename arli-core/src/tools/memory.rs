//! Memory tool — provides the agent with persistent cross-session memory.
//!
//! Actions: add, replace, remove, get, search, count.
//! Targets: "user" (user preferences/profile) or "memory" (agent notes).

use async_trait::async_trait;
use std::sync::Arc;

use crate::error::Result;
use crate::memory::MemoryStore;
use super::{Tool, ToolOutput};

/// Tool for managing persistent memories.
pub struct MemoryTool {
    store: Arc<MemoryStore>,
}

impl MemoryTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Save/retrieve durable information to persistent memory that survives across sessions. \
         Actions: add, replace, remove, get, search, count. \
         Targets: 'user' (user profile/preferences) or 'memory' (agent notes). \
         Use add to save new facts. Use replace when updating: provide old_text substring. \
         Use remove to delete. Get returns all memories for a target. Search does FTS5 lookup."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "replace", "remove", "get", "search", "count"],
                    "description": "The action to perform"
                },
                "target": {
                    "type": "string",
                    "enum": ["user", "memory"],
                    "description": "Which memory store: 'user' for profile/preferences, 'memory' for agent notes"
                },
                "content": {
                    "type": "string",
                    "description": "Content for 'add' or 'replace' actions"
                },
                "old_text": {
                    "type": "string",
                    "description": "For 'replace': substring of the old content to find and replace. For 'remove': exact content to delete."
                },
                "query": {
                    "type": "string",
                    "description": "FTS5 search query for 'search' action"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results for 'search' action",
                    "default": 10
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

        let result = match action {
            "add" => self.handle_add(&args),
            "replace" => self.handle_replace(&args),
            "remove" => self.handle_remove(&args),
            "get" => self.handle_get(&args),
            "search" => self.handle_search(&args),
            "count" => self.handle_count(),
            other => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Unknown action: {}. Valid: add, replace, remove, get, search, count", other)),
                }
            }
        };

        match result {
            Ok(content) => ToolOutput {
                success: true,
                content,
                error: None,
            },
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(e.to_string()),
            },
        }
    }
}

impl MemoryTool {
    fn handle_add(&self, args: &serde_json::Value) -> Result<String> {
        let target = require_str(args, "target")?;
        let content = require_str(args, "content")?;

        let id = self.store.add(target, content)?;
        Ok(format!("Memory saved: [{target}] #{id} ({:.100}...)", &content[..content.len().min(100)]))
    }

    fn handle_replace(&self, args: &serde_json::Value) -> Result<String> {
        let target = require_str(args, "target")?;
        let old_text = require_str(args, "old_text")?;
        let content = require_str(args, "content")?;

        let updated = self.store.replace(target, old_text, content)?;
        if updated > 0 {
            Ok(format!("Replaced {} memory entr{} in [{target}]", updated, if updated == 1 { "y" } else { "ies" }))
        } else {
            Ok(format!("No memory matching '{}' found in [{target}]", old_text))
        }
    }

    fn handle_remove(&self, args: &serde_json::Value) -> Result<String> {
        let target = require_str(args, "target")?;
        let old_text = require_str(args, "old_text")?;

        let removed = self.store.remove(target, old_text)?;
        Ok(format!("Removed {} memory entr{} from [{target}]", removed, if removed == 1 { "y" } else { "ies" }))
    }

    fn handle_get(&self, args: &serde_json::Value) -> Result<String> {
        let target = args["target"].as_str();

        let entries = self.store.get_all(target)?;
        if entries.is_empty() {
            return Ok("No memories found.".into());
        }

        let mut out = String::new();
        for entry in &entries {
            out.push_str(&format!(
                "[{}] #{:>3} | {} | {}\n",
                entry.target,
                entry.id,
                &entry.updated_at[..entry.updated_at.len().min(19)],
                entry.content
            ));
        }

        Ok(format!("{} memories:\n{}", entries.len(), out))
    }

    fn handle_search(&self, args: &serde_json::Value) -> Result<String> {
        let query = require_str(args, "query")?;
        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        let entries = self.store.search(query, limit)?;
        if entries.is_empty() {
            return Ok(format!("No memories matching '{}'", query));
        }

        let mut out = String::new();
        for entry in &entries {
            out.push_str(&format!(
                "[{}] #{} | {}\n",
                entry.target, entry.id, entry.content
            ));
        }

        Ok(format!("{} results for '{}':\n{}", entries.len(), query, out))
    }

    fn handle_count(&self) -> Result<String> {
        let (user_count, mem_count) = self.store.count()?;
        Ok(format!(
            "User memories: {}, Agent memories: {}, Total: {}",
            user_count,
            mem_count,
            user_count + mem_count
        ))
    }
}

fn require_str<'a>(args: &'a serde_json::Value, field: &str) -> Result<&'a str> {
    args[field]
        .as_str()
        .ok_or_else(|| crate::error::Error::Tool(format!("Missing required field: {}", field)))
}
