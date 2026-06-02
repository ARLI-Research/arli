//! Kanban tool — agent-facing interface to kanban boards.
//!
//! Agents use this tool to manage task boards: decompose work into cards,
//! track progress across columns, and self-manage their backlog.
//!
//! Actions: create_board, get_board, list_boards, delete_board,
//! add_card, move_card, update_card, delete_card, get_stats, add_column.

use async_trait::async_trait;
use std::sync::Arc;

use super::{Tool, ToolOutput};
use crate::kanban::{KanbanStore, Priority};

/// Tool for agent-driven kanban board management.
pub struct KanbanTool {
    store: Arc<KanbanStore>,
}

impl KanbanTool {
    pub fn new(store: Arc<KanbanStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for KanbanTool {
    fn name(&self) -> &str {
        "kanban"
    }

    fn description(&self) -> &str {
        "Manage kanban task boards: create boards, add/move/update/delete cards, \
         view board stats. Use this to decompose work, track progress, and self-manage \
         your backlog. Actions: create_board, get_board, list_boards, delete_board, \
         add_card, move_card, update_card, delete_card, get_stats, add_column."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "create_board", "get_board", "list_boards", "delete_board",
                        "add_card", "move_card", "update_card", "delete_card",
                        "get_stats", "add_column"
                    ],
                    "description": "The action to perform"
                },
                "board_id": {
                    "type": "string",
                    "description": "Board ID (required for most actions)"
                },
                "board_name": {
                    "type": "string",
                    "description": "Board name (for create_board)"
                },
                "board_description": {
                    "type": "string",
                    "description": "Board description (for create_board)"
                },
                "column_id": {
                    "type": "string",
                    "description": "Column ID (required for add_card, move_card)"
                },
                "column_name": {
                    "type": "string",
                    "description": "Column name (for add_column)"
                },
                "wip_limit": {
                    "type": "integer",
                    "description": "WIP limit for column (for add_column)"
                },
                "card_id": {
                    "type": "string",
                    "description": "Card ID (required for move_card, update_card, delete_card)"
                },
                "title": {
                    "type": "string",
                    "description": "Card title"
                },
                "description": {
                    "type": "string",
                    "description": "Card description"
                },
                "priority": {
                    "type": "string",
                    "enum": ["critical", "high", "medium", "low"],
                    "description": "Card priority"
                },
                "assignee": {
                    "type": "string",
                    "description": "Assignee name/ID"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tags for the card"
                },
                "due_date": {
                    "type": "string",
                    "description": "Due date (YYYY-MM-DD format)"
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
                    error: Some(format!("Invalid JSON: {e}")),
                }
            }
        };

        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let result = match action {
            "create_board" => self.create_board(&args),
            "get_board" => self.get_board(&args),
            "list_boards" => self.list_boards(),
            "delete_board" => self.delete_board(&args),
            "add_column" => self.add_column(&args),
            "add_card" => self.add_card(&args),
            "move_card" => self.move_card(&args),
            "update_card" => self.update_card(&args),
            "delete_card" => self.delete_card(&args),
            "get_stats" => self.get_stats(&args),
            _ => Err(format!("Unknown action: {action}")),
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
                error: Some(e),
            },
        }
    }
}

impl KanbanTool {
    fn get_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
        args.get(key).and_then(|v| v.as_str())
    }

    fn create_board(&self, args: &serde_json::Value) -> std::result::Result<String, String> {
        let name = Self::get_str(args, "board_name").ok_or("board_name is required")?;
        let desc = Self::get_str(args, "board_description").unwrap_or("");
        self.store
            .create_board(name, desc)
            .map(|b| serde_json::to_string_pretty(&b).unwrap_or_default())
            .map_err(|e| e.to_string())
    }

    fn get_board(&self, args: &serde_json::Value) -> std::result::Result<String, String> {
        let id = Self::get_str(args, "board_id").ok_or("board_id is required")?;
        self.store
            .get_board(id)
            .map(|b| serde_json::to_string_pretty(&b).unwrap_or_default())
            .map_err(|e| e.to_string())
    }

    fn list_boards(&self) -> std::result::Result<String, String> {
        self.store
            .list_boards()
            .map(|boards| serde_json::to_string_pretty(&boards).unwrap_or_default())
            .map_err(|e| e.to_string())
    }

    fn delete_board(&self, args: &serde_json::Value) -> std::result::Result<String, String> {
        let id = Self::get_str(args, "board_id").ok_or("board_id is required")?;
        self.store.delete_board(id).map_err(|e| e.to_string())?;
        Ok(format!("Board {id} deleted"))
    }

    fn add_column(&self, args: &serde_json::Value) -> std::result::Result<String, String> {
        let board_id = Self::get_str(args, "board_id").ok_or("board_id is required")?;
        let name = Self::get_str(args, "column_name").ok_or("column_name is required")?;
        let wip_limit = args
            .get("wip_limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        self.store
            .add_column(board_id, name, wip_limit)
            .map(|c| serde_json::to_string_pretty(&c).unwrap_or_default())
            .map_err(|e| e.to_string())
    }

    fn add_card(&self, args: &serde_json::Value) -> std::result::Result<String, String> {
        let board_id = Self::get_str(args, "board_id").ok_or("board_id is required")?;
        let column_id = Self::get_str(args, "column_id").ok_or("column_id is required")?;
        let title = Self::get_str(args, "title").ok_or("title is required")?;
        let description = Self::get_str(args, "description").unwrap_or("");
        let priority = Priority::from_str(Self::get_str(args, "priority").unwrap_or("medium"));
        let assignee = Self::get_str(args, "assignee");
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let due_date = Self::get_str(args, "due_date");

        self.store
            .add_card(
                board_id,
                column_id,
                title,
                description,
                priority,
                assignee,
                &tags,
                due_date,
            )
            .map(|c| serde_json::to_string_pretty(&c).unwrap_or_default())
            .map_err(|e| e.to_string())
    }

    fn move_card(&self, args: &serde_json::Value) -> std::result::Result<String, String> {
        let card_id = Self::get_str(args, "card_id").ok_or("card_id is required")?;
        let column_id = Self::get_str(args, "column_id").ok_or("column_id is required")?;
        self.store
            .move_card(card_id, column_id)
            .map(|c| serde_json::to_string_pretty(&c).unwrap_or_default())
            .map_err(|e| e.to_string())
    }

    fn update_card(&self, args: &serde_json::Value) -> std::result::Result<String, String> {
        let card_id = Self::get_str(args, "card_id").ok_or("card_id is required")?;
        let title = Self::get_str(args, "title");
        let description = Self::get_str(args, "description");
        let priority = Self::get_str(args, "priority");

        // Parse assignee: None means not provided; Some(None) means explicit null
        let assignee = args.get("assignee").map(|v| {
            if v.is_null() {
                None
            } else {
                v.as_str().map(|s| s.to_string())
            }
        });
        let assignee_ref: Option<Option<&str>> = assignee.as_ref().map(|inner| inner.as_deref());

        // Parse tags: None means not provided
        let tags: Option<Vec<String>> = args.get("tags").map(|v| {
            v.as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        });
        let tags_ref: Option<&[String]> = tags.as_deref();

        // Parse due_date: None means not provided; Some(None) means explicit null
        let due_date = args.get("due_date").map(|v| {
            if v.is_null() {
                None
            } else {
                v.as_str().map(|s| s.to_string())
            }
        });
        let due_date_ref: Option<Option<&str>> = due_date.as_ref().map(|inner| inner.as_deref());

        self.store
            .update_card(
                card_id,
                title,
                description,
                priority,
                assignee_ref,
                tags_ref,
                due_date_ref,
            )
            .map(|c| serde_json::to_string_pretty(&c).unwrap_or_default())
            .map_err(|e| e.to_string())
    }

    fn delete_card(&self, args: &serde_json::Value) -> std::result::Result<String, String> {
        let card_id = Self::get_str(args, "card_id").ok_or("card_id is required")?;
        self.store.delete_card(card_id).map_err(|e| e.to_string())?;
        Ok(format!("Card {card_id} deleted"))
    }

    fn get_stats(&self, args: &serde_json::Value) -> std::result::Result<String, String> {
        let board_id = Self::get_str(args, "board_id").ok_or("board_id is required")?;
        self.store
            .get_board_stats(board_id)
            .map(|s| serde_json::to_string_pretty(&s).unwrap_or_default())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tool() -> KanbanTool {
        let store = KanbanStore::open_in_memory().unwrap();
        KanbanTool::new(Arc::new(store))
    }

    #[tokio::test]
    async fn test_create_board_via_tool() {
        let tool = test_tool();
        let output = tool
            .execute(
                r#"{"action": "create_board", "board_name": "Sprint 42", "board_description": "The answer sprint"}"#,
            )
            .await;

        assert!(output.success, "{}", output.error.unwrap_or_default());
        let board: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(board["name"], "Sprint 42");
        assert!(!board["id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_full_card_lifecycle_via_tool() {
        let tool = test_tool();

        // Create board
        let output = tool
            .execute(r#"{"action": "create_board", "board_name": "Lifecycle"}"#)
            .await;
        assert!(output.success);
        let board: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        let board_id = board["id"].as_str().unwrap();

        // Get stats to find columns
        let output = tool
            .execute(&format!(
                r#"{{"action": "get_stats", "board_id": "{}"}}"#,
                board_id
            ))
            .await;
        assert!(output.success);
        let stats: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        let backlog_id = stats["columns"][0]["column_id"].as_str().unwrap();
        let todo_id = stats["columns"][1]["column_id"].as_str().unwrap();

        // Add card
        let output = tool
            .execute(&format!(
                r#"{{"action": "add_card", "board_id": "{}", "column_id": "{}", "title": "Fix SSL", "description": "Cert expired", "priority": "high", "tags": ["bug", "security"]}}"#,
                board_id, backlog_id
            ))
            .await;
        assert!(output.success);
        let card: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        let card_id = card["id"].as_str().unwrap();
        assert_eq!(card["title"], "Fix SSL");
        assert_eq!(card["priority"], "high");

        // Move card to todo
        let output = tool
            .execute(&format!(
                r#"{{"action": "move_card", "card_id": "{}", "column_id": "{}"}}"#,
                card_id, todo_id
            ))
            .await;
        assert!(output.success);
        let moved: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(moved["column_id"], todo_id);

        // Update card
        let output = tool
            .execute(&format!(
                r#"{{"action": "update_card", "card_id": "{}", "title": "Fix SSL cert", "priority": "critical"}}"#,
                card_id
            ))
            .await;
        assert!(output.success);
        let updated: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(updated["title"], "Fix SSL cert");
        assert_eq!(updated["priority"], "critical");

        // Delete card
        let output = tool
            .execute(&format!(
                r#"{{"action": "delete_card", "card_id": "{}"}}"#,
                card_id
            ))
            .await;
        assert!(output.success);

        // Verify stats updated
        let output = tool
            .execute(&format!(
                r#"{{"action": "get_stats", "board_id": "{}"}}"#,
                board_id
            ))
            .await;
        let stats: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(stats["total_cards"], 0);
    }

    #[tokio::test]
    async fn test_tool_error_handling() {
        let tool = test_tool();

        // Missing required field
        let output = tool.execute(r#"{"action": "create_board"}"#).await;
        assert!(!output.success);

        // Unknown action
        let output = tool.execute(r#"{"action": "fly_to_moon"}"#).await;
        assert!(!output.success);

        // Invalid JSON
        let output = tool.execute("not json").await;
        assert!(!output.success);
    }
}
