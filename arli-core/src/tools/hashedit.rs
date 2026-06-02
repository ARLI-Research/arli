//! HashEdit tool — content-hash anchored file editing.
//!
//! Uses hashline anchors instead of retyping file content. The model
//! identifies target lines by their content hash, eliminating whitespace
//! mismatches and "string not found" errors.

use super::{Tool, ToolOutput};
use async_trait::async_trait;

pub struct HashEditTool;

#[async_trait]
impl Tool for HashEditTool {
    fn name(&self) -> &str {
        "hashedit"
    }

    fn description(&self) -> &str {
        "Edit a file using content-hash anchors. \
         Provide the file path and a list of hunks. Each hunk uses \
         'before'/'after' anchors (8-char line hashes) to locate the \
         target region, then 'remove' and 'insert' to specify changes. \
         More reliable than patch — no whitespace battles."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "hunks": {
                    "type": "array",
                    "description": "List of edit hunks to apply",
                    "items": {
                        "type": "object",
                        "properties": {
                            "before": {
                                "type": "array",
                                "description": "Anchor hashes before the edit",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "hash": {"type": "string", "description": "8-char line hash"},
                                        "line_hint": {"type": "integer", "description": "Line number hint"}
                                    }
                                }
                            },
                            "after": {
                                "type": "array",
                                "description": "Anchor hashes after the edit"
                            },
                            "remove": {
                                "type": "array",
                                "description": "Lines to remove (exact content)",
                                "items": {"type": "string"}
                            },
                            "insert": {
                                "type": "array",
                                "description": "Lines to insert",
                                "items": {"type": "string"}
                            },
                            "target_hash": {
                                "type": "string",
                                "description": "Precise hash of the line to replace"
                            }
                        },
                        "required": ["before"]
                    }
                }
            },
            "required": ["path", "hunks"]
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

        let path = match args["path"].as_str() {
            Some(p) => p,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'path'".into()),
                }
            }
        };

        let old_content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Cannot read '{path}': {e}")),
                }
            }
        };

        let hunks: Vec<crate::hashline::Hunk> = match serde_json::from_value(
            args["hunks"].clone(),
        ) {
            Ok(h) => h,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Invalid hunks: {e}")),
                }
            }
        };

        if hunks.is_empty() {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some("No hunks provided.".into()),
            };
        }

        match crate::hashline::apply_hunks(&old_content, &hunks) {
            Ok(new_content) => {
                if let Err(e) = std::fs::write(path, &new_content) {
                    return ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some(format!("Cannot write '{path}': {e}")),
                    };
                }

                // Count changes
                let old_lines = old_content.lines().count();
                let new_lines = new_content.lines().count();
                let changed_files = 1;
                let replacements = hunks.len();

                ToolOutput {
                    success: true,
                    content: format!(
                        "Applied {replacements} replacement(s) in '{path}' \
                         ({old_lines} lines → {new_lines} lines)."
                    ),
                    error: None,
                }
            }
            Err(e) => {
                // If anchors are stale, include current hashes so the model can retry
                let hint = if e.contains("file has changed") {
                    let hashes = crate::hashline::hash_lines(&old_content);
                    format!(
                        "\n\nCurrent file hashes (use these for new anchors):\n{:?}",
                        hashes.iter().take(20).collect::<Vec<_>>()
                    )
                } else {
                    String::new()
                };

                ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("{e}{hint}")),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hashedit_simple_replace() {
        let path = "/tmp/arli-hashedit-test.txt";
        std::fs::write(path, "fn main() {\n    let x = 42;\n}\n").unwrap();

        let target_hash = crate::hashline::hash_line("    let x = 42;");

        let tool = HashEditTool;
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": path,
                    "hunks": [{
                        "before": [{"hash": target_hash}],
                        "after": [],
                        "remove": ["    let x = 42;"],
                        "insert": ["    let x = 99;"],
                        "target_hash": target_hash
                    }]
                })
                .to_string(),
            )
            .await;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("let x = 99;"));
        assert!(!content.contains("let x = 42;"));

        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn test_hashedit_stale_anchor() {
        let path = "/tmp/arli-hashedit-stale.txt";
        std::fs::write(path, "hello\nworld\n").unwrap();

        let tool = HashEditTool;
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": path,
                    "hunks": [{
                        "before": [{"hash": "deadbeef"}],
                        "after": [],
                        "remove": [],
                        "insert": ["new line"]
                    }]
                })
                .to_string(),
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("file has changed"));

        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn test_hashedit_insert_only() {
        let path = "/tmp/arli-hashedit-insert.txt";
        std::fs::write(path, "line one\nline two\n").unwrap();

        let anchor = crate::hashline::hash_line("line one");

        let tool = HashEditTool;
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": path,
                    "hunks": [{
                        "before": [{"hash": anchor}],
                        "after": [],
                        "remove": [],
                        "insert": ["inserted line"]
                    }]
                })
                .to_string(),
            )
            .await;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("inserted line"));
        assert!(content.contains("line one"));

        std::fs::remove_file(path).ok();
    }
}
