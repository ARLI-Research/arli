//! Resolve tool — accept or reject staged edits (preview-then-accept).
//!
//! Works with `ast_edit` (and potentially other staging tools in the future).
//! Each staged edit has an ID that the agent references to accept or reject it.
//!
//! ## Workflow
//!
//! 1. `ast_edit` with `action: "replace"` → returns proposed diff + staged ID
//! 2. Agent reviews the diff
//! 3. `resolve` with `action: "accept"` or `action: "reject"` → commits or discards

use super::ast_edit::{load_staged, remove_staged};
use super::{Tool, ToolOutput};
use async_trait::async_trait;
use std::path::PathBuf;

pub struct ResolveTool {
    /// Directory where staged edits are stored
    stage_dir: PathBuf,
}

impl ResolveTool {
    pub fn new(stage_dir: PathBuf) -> Self {
        Self { stage_dir }
    }
}

#[async_trait]
impl Tool for ResolveTool {
    fn name(&self) -> &str {
        "resolve"
    }

    fn description(&self) -> &str {
        "Accept or reject a staged edit. After 'ast_edit' with action='replace', \
         the edit is staged for preview. Use resolve with action='accept' to commit \
         the change to disk, or action='reject' to discard it. \
         Provide a reason for audit trail."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "staged_id": {
                    "type": "string",
                    "description": "The staged edit ID returned by ast_edit"
                },
                "action": {
                    "type": "string",
                    "description": "'accept' to apply the change, 'reject' to discard it",
                    "enum": ["accept", "reject"]
                },
                "reason": {
                    "type": "string",
                    "description": "Why the edit is accepted or rejected (for audit trail)"
                }
            },
            "required": ["staged_id", "action", "reason"]
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

        let staged_id = match args["staged_id"].as_str() {
            Some(id) => id,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: staged_id".into()),
                }
            }
        };

        let action = match args["action"].as_str() {
            Some(a) => a,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: action".into()),
                }
            }
        };

        let reason = args["reason"].as_str().unwrap_or("(no reason provided)");

        // Load the staged edit
        let staged = match load_staged(&self.stage_dir, staged_id) {
            Some(s) => s,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!(
                        "Staged edit '{staged_id}' not found. It may have already been resolved or expired."
                    )),
                }
            }
        };

        match action {
            "accept" => {
                // Apply the edit
                match std::fs::write(&staged.path, &staged.new_content) {
                    Ok(_) => {
                        // Remove the staged file
                        remove_staged(&self.stage_dir, staged_id);

                        // Count old vs new lines
                        let old_src = std::fs::read_to_string(&staged.path).unwrap_or_default();
                        let old_lines = old_src.lines().count();
                        let new_lines = staged.new_content.lines().count();

                        ToolOutput {
                            success: true,
                            content: format!(
                                "Accepted: {replacements} replacement(s) in '{path}' \
                                 ({old_lines} lines → {new_lines} lines)\n\
                                 Reason: {reason}",
                                replacements = staged.replacements,
                                path = staged.path,
                            ),
                            error: None,
                        }
                    }
                    Err(e) => ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some(format!("Cannot write '{}': {e}", staged.path)),
                    },
                }
            }

            "reject" => {
                // Discard the staged edit
                remove_staged(&self.stage_dir, staged_id);

                ToolOutput {
                    success: true,
                    content: format!(
                        "Rejected: {replacements} proposed replacement(s) in '{path}' \
                         discarded.\nReason: {reason}",
                        replacements = staged.replacements,
                        path = staged.path,
                    ),
                    error: None,
                }
            }

            other => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!(
                    "Unknown action: '{other}'. Use 'accept' or 'reject'."
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ast_edit::AstEditTool;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_resolve_accept() {
        let dir = TempDir::new().unwrap();
        let stage_dir = dir.path().join("stage");
        let file_path = dir.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {\n    let x = 42;\n}\n").unwrap();

        // Stage an edit
        let ast_tool = AstEditTool::new(stage_dir.clone());
        let ast_result = ast_tool
            .execute(
                &serde_json::json!({
                    "path": file_path.to_str().unwrap(),
                    "pattern": "let $A = $B",
                    "replacement": "let $A: i32 = $B",
                    "action": "replace"
                })
                .to_string(),
            )
            .await;

        assert!(ast_result.success);
        // Extract staged_id from "Staged as `{id}`."
        let staged_id = {
            let re = regex::Regex::new(r"Staged as `([^`]+)`").unwrap();
            re.captures(&ast_result.content)
                .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
                .expect("No staged ID found")
        };

        // Accept the edit
        let resolve_tool = ResolveTool::new(stage_dir);
        let result = resolve_tool
            .execute(
                &serde_json::json!({
                    "staged_id": staged_id,
                    "action": "accept",
                    "reason": "Adding type annotation"
                })
                .to_string(),
            )
            .await;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(result.content.contains("Accepted"));

        // Verify file was modified
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(
            content.contains("let x: i32 = 42"),
            "File not modified: {}",
            content
        );
    }

    #[tokio::test]
    async fn test_resolve_reject() {
        let dir = TempDir::new().unwrap();
        let stage_dir = dir.path().join("stage");
        let file_path = dir.path().join("test.rs");
        let original = "fn main() {\n    let x = 42;\n}\n";
        std::fs::write(&file_path, original).unwrap();

        let ast_tool = AstEditTool::new(stage_dir.clone());
        let ast_result = ast_tool
            .execute(
                &serde_json::json!({
                    "path": file_path.to_str().unwrap(),
                    "pattern": "let $A = $B",
                    "replacement": "let $A: i32 = $B",
                    "action": "replace"
                })
                .to_string(),
            )
            .await;

        assert!(ast_result.success);
        // Extract staged_id from "Staged as `{id}`."
        let staged_id = {
            let re = regex::Regex::new(r"Staged as `([^`]+)`").unwrap();
            re.captures(&ast_result.content)
                .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
                .expect("No staged ID found")
        };

        let resolve_tool = ResolveTool::new(stage_dir);
        let result = resolve_tool
            .execute(
                &serde_json::json!({
                    "staged_id": staged_id,
                    "action": "reject",
                    "reason": "Don't need type annotations here"
                })
                .to_string(),
            )
            .await;

        assert!(result.success);
        assert!(result.content.contains("Rejected"));

        // File should NOT be modified
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, original, "File was modified but should not be");
    }

    #[tokio::test]
    async fn test_resolve_not_found() {
        let dir = TempDir::new().unwrap();
        let stage_dir = dir.path().join("stage");
        let resolve_tool = ResolveTool::new(stage_dir);

        let result = resolve_tool
            .execute(
                &serde_json::json!({
                    "staged_id": "edit_nonexistent",
                    "action": "accept",
                    "reason": "test"
                })
                .to_string(),
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }
}
