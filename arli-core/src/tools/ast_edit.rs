//! AST Edit tool — structural code edits via ast-grep.
//!
//! Uses tree-sitter grammars for precise structural matching and replacement.
//! Edits are staged for preview — the agent must call `resolve` to commit them.
//!
//! ## Two modes
//!
//! - `action: "find"` — return matching AST nodes (no file change)
//! - `action: "replace"` — compute replacements, stage them, return diff

use super::{Tool, ToolOutput};
use crate::ast_language::AstLanguage;
use async_trait::async_trait;
use std::path::PathBuf;

pub struct AstEditTool {
    /// Directory for staging edits (preview-then-accept)
    stage_dir: PathBuf,
}

impl AstEditTool {
    pub fn new(stage_dir: PathBuf) -> Self {
        Self { stage_dir }
    }
}

#[async_trait]
impl Tool for AstEditTool {
    fn name(&self) -> &str {
        "ast_edit"
    }

    fn description(&self) -> &str {
        "Structural code edits via AST pattern matching (ast-grep). \
         Use 'action: find' to discover matches, 'action: replace' to \
         compute replacements. Replacements are staged — use 'resolve' \
         tool to accept or reject them. \
         Supported languages: rust, python, typescript, javascript."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "lang": {
                    "type": "string",
                    "description": "Language: rust, python, typescript, javascript. Auto-detected from extension if omitted.",
                    "enum": ["rust", "python", "typescript", "javascript"]
                },
                "pattern": {
                    "type": "string",
                    "description": "AST pattern to match. Use $VAR for metavariables. Examples: 'console.log($X)', 'let $A = $B', 'def $NAME($PARAMS)'"
                },
                "replacement": {
                    "type": "string",
                    "description": "Replacement string. Use same $VAR names as pattern. Required when action='replace'."
                },
                "action": {
                    "type": "string",
                    "description": "'find' returns matches without editing; 'replace' stages changes (use 'resolve' to commit).",
                    "enum": ["find", "replace"],
                    "default": "find"
                }
            },
            "required": ["path", "pattern"]
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

        let path_str = match args["path"].as_str() {
            Some(p) => p,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: path".into()),
                }
            }
        };

        let src = match std::fs::read_to_string(path_str) {
            Ok(s) => s,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Cannot read '{path_str}': {e}")),
                }
            }
        };

        // Resolve language
        let lang = if let Some(lang_str) = args["lang"].as_str() {
            match lang_str {
                "rust" => AstLanguage::Rust,
                "python" => AstLanguage::Python,
                "typescript" => AstLanguage::TypeScript,
                "javascript" => AstLanguage::JavaScript,
                other => {
                    return ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some(format!(
                            "Unsupported language: '{other}'. Supported: {}",
                            AstLanguage::supported_langs().join(", ")
                        )),
                    }
                }
            }
        } else {
            // Auto-detect from file extension
            let path = std::path::Path::new(path_str);
            match AstLanguage::from_path(path) {
                Some(l) => l,
                None => {
                    return ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some(format!(
                            "Cannot auto-detect language for '{path_str}'. \
                             Provide 'lang' explicitly. Supported: {}",
                            AstLanguage::supported_langs().join(", ")
                        )),
                    }
                }
            }
        };

        let pattern_str = match args["pattern"].as_str() {
            Some(p) => p,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: pattern".into()),
                }
            }
        };

        let action = args["action"].as_str().unwrap_or("find");

        // Parse source and pattern
        let root = ast_grep_core::AstGrep::new(src.as_str(), lang.clone());

        match action {
            "find" => {
                // Find all matches — find_all returns an iterator, not a Result
                let matches: Vec<_> = root.root().find_all(pattern_str).collect();
                if matches.is_empty() {
                    ToolOutput {
                        success: true,
                        content: format!(
                            "No matches found for pattern '{pattern_str}' in '{path_str}'"
                        ),
                        error: None,
                    }
                } else {
                    let mut output = format!(
                        "Found {} match(es) for pattern '{pattern_str}' in '{path_str}':\n\n",
                        matches.len()
                    );
                    for (i, m) in matches.iter().enumerate() {
                        let pos = m.start_pos();
                        let col = pos.column(m.get_node());
                        let text = m.text();
                        output.push_str(&format!(
                            "  [{i}] L{}:C{} — `{text}`\n",
                            pos.line() + 1,
                            col + 1,
                        ));
                    }
                    ToolOutput {
                        success: true,
                        content: output,
                        error: None,
                    }
                }
            }

            "replace" => {
                let replacement_str = match args["replacement"].as_str() {
                    Some(r) => r,
                    None => {
                        return ToolOutput {
                            success: false,
                            content: String::new(),
                            error: Some("'replacement' is required when action='replace'".into()),
                        }
                    }
                };

                // First: find matches (for counting and staging)
                let matches: Vec<_> = root.root().find_all(pattern_str).collect();

                if matches.is_empty() {
                    return ToolOutput {
                        success: true,
                        content: format!(
                            "No matches found for pattern '{pattern_str}' in '{path_str}' — nothing to replace."
                        ),
                        error: None,
                    };
                }

                let match_count = matches.len();

                // Perform replacement in a separate tree
                let mut ast = ast_grep_core::AstGrep::new(src.as_str(), lang);
                let replaced = match ast.replace(pattern_str, replacement_str) {
                    Ok(r) => r,
                    Err(e) => {
                        return ToolOutput {
                            success: false,
                            content: String::new(),
                            error: Some(format!("Replace error: {e}")),
                        }
                    }
                };

                if !replaced {
                    return ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some("Replace returned false — no replacements applied.".into()),
                    };
                }

                let new_content = ast.generate();

                // Compute diff
                let diff = compute_diff(&src, &new_content, path_str);

                // Stage the edit
                let staged_id = stage_edit(
                    &self.stage_dir,
                    path_str,
                    &new_content,
                    &diff,
                    match_count,
                    pattern_str,
                    replacement_str,
                );

                ToolOutput {
                    success: true,
                    content: format!(
                        "AST Edit: `{pattern_str}` → `{replacement_str}` (proposed)\n\
                         {match_count} replacement(s) in '{path_str}'\n\n\
                         ```diff\n{diff}\n```\n\n\
                         Staged as `{staged_id}`. Use resolve to accept or reject."
                    ),
                    error: None,
                }
            }

            other => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!(
                    "Unknown action: '{other}'. Use 'find' or 'replace'."
                )),
            },
        }
    }
}

/// Compute a simple unified diff between two strings.
fn compute_diff(old: &str, new: &str, _path: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Simple diff: show changed lines with context
    let mut result = String::new();

    let min_len = old_lines.len().min(new_lines.len());
    let mut i = 0;

    // Find first difference
    while i < min_len && old_lines[i] == new_lines[i] {
        i += 1;
    }

    // Find last difference (from the end)
    let mut old_end = old_lines.len();
    let mut new_end = new_lines.len();
    while old_end > i && new_end > i && old_lines[old_end - 1] == new_lines[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }

    // Emit context before
    let context_start = i.saturating_sub(2);
    for line in old_lines[context_start..i].iter() {
        result.push_str(&format!("  {line}\n"));
    }

    // Emit removed lines
    for line in old_lines[i..old_end].iter() {
        result.push_str(&format!("- {line}\n"));
    }

    // Emit added lines
    for line in new_lines[i..new_end].iter() {
        result.push_str(&format!("+ {line}\n"));
    }

    // Emit context after
    let context_end = (new_end + 2).min(new_lines.len());
    for line in new_lines[new_end..context_end].iter() {
        result.push_str(&format!("  {line}\n"));
    }

    if result.is_empty() {
        result.push_str("(no changes)");
    }

    result
}

/// Serialized staged edit — saved to disk for later resolution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StagedEdit {
    pub id: String,
    pub path: String,
    pub new_content: String,
    pub diff: String,
    pub replacements: usize,
    pub pattern: String,
    pub replacement: String,
}

/// Stage an edit to disk for preview-then-accept.
fn stage_edit(
    stage_dir: &std::path::Path,
    path: &str,
    new_content: &str,
    diff: &str,
    replacements: usize,
    pattern: &str,
    replacement: &str,
) -> String {
    let id = format!("edit_{}", &uuid::Uuid::new_v4().to_string()[..8]);

    // Ensure stage directory exists
    std::fs::create_dir_all(stage_dir).ok();

    let staged = StagedEdit {
        id: id.clone(),
        path: path.to_string(),
        new_content: new_content.to_string(),
        diff: diff.to_string(),
        replacements,
        pattern: pattern.to_string(),
        replacement: replacement.to_string(),
    };

    let stage_path = stage_dir.join(format!("{}.json", id));
    if let Ok(json) = serde_json::to_string_pretty(&staged) {
        std::fs::write(&stage_path, json).ok();
    }

    id
}

/// Load a staged edit from disk.
pub fn load_staged(stage_dir: &std::path::Path, staged_id: &str) -> Option<StagedEdit> {
    let stage_path = stage_dir.join(format!("{staged_id}.json"));
    let json = std::fs::read_to_string(&stage_path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Remove a staged edit from disk.
pub fn remove_staged(stage_dir: &std::path::Path, staged_id: &str) {
    let stage_path = stage_dir.join(format!("{staged_id}.json"));
    std::fs::remove_file(stage_path).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_ast_edit_find_rust() {
        let dir = TempDir::new().unwrap();
        let stage_dir = dir.path().join("stage");
        let file_path = dir.path().join("test.rs");
        std::fs::write(
            &file_path,
            "fn main() {\n    let x = 42;\n    let y = 99;\n}\n",
        )
        .unwrap();

        let tool = AstEditTool::new(stage_dir);
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": file_path.to_str().unwrap(),
                    "pattern": "let $A = $B",
                    "action": "find"
                })
                .to_string(),
            )
            .await;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(
            result.content.contains("2 match"),
            "Got: {}",
            result.content
        );
        assert!(result.content.contains("let x = 42"));
        assert!(result.content.contains("let y = 99"));
    }

    #[tokio::test]
    async fn test_ast_edit_replace_rust() {
        let dir = TempDir::new().unwrap();
        let stage_dir = dir.path().join("stage");
        let file_path = dir.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {\n    let x = 42;\n}\n").unwrap();

        let tool = AstEditTool::new(stage_dir);
        let result = tool
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

        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(
            result.content.contains("(proposed)"),
            "Got: {}",
            result.content
        );
        assert!(
            result.content.contains("Staged as"),
            "Got: {}",
            result.content
        );
        // File should NOT be modified yet
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(
            content.contains("let x = 42"),
            "File was modified before resolve: {}",
            content
        );
        assert!(!content.contains("let x: i32 = 42"));
    }

    #[tokio::test]
    async fn test_ast_edit_find_js() {
        let dir = TempDir::new().unwrap();
        let stage_dir = dir.path().join("stage");
        let file_path = dir.path().join("test.js");
        std::fs::write(&file_path, "function foo(a, b) { return a + b; }\n").unwrap();

        let tool = AstEditTool::new(stage_dir);
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": file_path.to_str().unwrap(),
                    "pattern": "return $EXPR",
                    "action": "find"
                })
                .to_string(),
            )
            .await;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(
            result.content.contains("return a + b"),
            "Got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_ast_edit_no_matches() {
        let dir = TempDir::new().unwrap();
        let stage_dir = dir.path().join("stage");
        let file_path = dir.path().join("test.rs");
        std::fs::write(&file_path, "struct Foo { x: i32 }\n").unwrap();

        let tool = AstEditTool::new(stage_dir);
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": file_path.to_str().unwrap(),
                    "pattern": "console.log($X)",
                    "action": "find"
                })
                .to_string(),
            )
            .await;

        assert!(result.success);
        assert!(result.content.contains("No matches found"));
    }
}
