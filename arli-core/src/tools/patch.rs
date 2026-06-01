//! Patch tool — targeted find-and-replace edits in files.
//!
//! Exactly what every coding agent needs: replace a specific string
//! in a file without rewriting the whole thing. Supports replace_all
//! mode for bulk changes.

use super::{Tool, ToolOutput};
use async_trait::async_trait;

pub struct PatchTool;

#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Make targeted edits to a file by finding and replacing a specific string. \
         Use this instead of write_file for small changes. \
         Set replace_all=true to replace all occurrences. \
         Returns the diff of changes made."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact text to find and replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text (pass empty string to delete)"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences instead of just the first",
                    "default": false
                }
            },
            "required": ["path", "old_string", "new_string"]
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

        let old_string = match args["old_string"].as_str() {
            Some(s) => s,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing 'old_string'".into()),
                }
            }
        };

        let new_string = args["new_string"].as_str().unwrap_or("");
        let replace_all = args["replace_all"].as_bool().unwrap_or(false);

        // Read the file
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Cannot read '{}': {}", path, e)),
                }
            }
        };

        // Find and replace
        if replace_all {
            let count = content.matches(old_string).count();
            if count == 0 {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!(
                        "String not found in '{}'. Check the exact text (whitespace, indentation).",
                        path
                    )),
                };
            }

            let new_content = content.replace(old_string, new_string);

            // Write back
            if let Err(e) = std::fs::write(path, &new_content) {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Cannot write '{}': {}", path, e)),
                };
            }

            // Generate a simple diff
            let diff = generate_diff(&content, &new_content, path);

            ToolOutput {
                success: true,
                content: format!(
                    "Replaced {} occurrence(s) in '{}'\n\nDiff:\n{}",
                    count, path, diff
                ),
                error: None,
            }
        } else {
            // Single replacement — must find exactly one match
            let count = content.matches(old_string).count();
            if count == 0 {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!(
                        "String not found in '{}'. Use more context to make the match unique.",
                        path
                    )),
                };
            }
            if count > 1 {
                let positions: Vec<_> = content
                    .match_indices(old_string)
                    .map(|(pos, _)| {
                        let line = content[..pos].lines().count() + 1;
                        format!("  line {}", line)
                    })
                    .take(5)
                    .collect();

                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!(
                        "Found {} matches in '{}'. Use more surrounding context \
                         to make the match unique, or set replace_all=true.\n\
                         First {} positions:\n{}",
                        count,
                        path,
                        positions.len().min(5),
                        positions.join("\n")
                    )),
                };
            }

            let new_content = content.replacen(old_string, new_string, 1);

            if let Err(e) = std::fs::write(path, &new_content) {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Cannot write '{}': {}", path, e)),
                };
            }

            let diff = generate_diff(&content, &new_content, path);

            ToolOutput {
                success: true,
                content: format!("Replaced in '{}'\n\n{}", path, diff),
                error: None,
            }
        }
    }
}

/// Generate a simple unified diff between old and new content.
fn generate_diff(old: &str, new: &str, path: &str) -> String {
    let mut diff = String::new();
    diff.push_str(&format!("--- {}\n+++ {}\n", path, path));

    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Very simple line-by-line diff
    let max_len = old_lines.len().max(new_lines.len());
    let mut changed = false;
    let mut hunk_start;

    for i in 0..max_len {
        let old_line = old_lines.get(i).copied();
        let new_line = new_lines.get(i).copied();

        if old_line != new_line {
            if !changed {
                hunk_start = Some(i.saturating_sub(2));
                changed = true;
                if let Some(start) = hunk_start {
                    diff.push_str(&format!(
                        "@@ -{},{} +{},{} @@\n",
                        start + 1,
                        max_len - start,
                        start + 1,
                        max_len - start
                    ));
                }
            }
            if let Some(line) = old_line {
                diff.push_str(&format!("-{}\n", line));
            }
            if let Some(line) = new_line {
                diff.push_str(&format!("+{}\n", line));
            }
        } else if changed && old_line == new_line {
            // Context line after a change
            diff.push_str(&format!(" {}\n", old_line.unwrap_or("")));
        }
    }

    if !changed {
        diff.push_str("(no changes detected)\n");
    }

    // Truncate if too long
    if diff.len() > 2000 {
        diff.truncate(2000);
        diff.push_str("\n... (diff truncated)");
    }

    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_patch_single_replace() {
        let path = "/tmp/arli-patch-test.txt";
        std::fs::write(path, "hello world\nfoo bar\n").unwrap();

        let tool = PatchTool;
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": path,
                    "old_string": "foo bar",
                    "new_string": "baz qux"
                })
                .to_string(),
            )
            .await;

        assert!(result.success);
        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(content, "hello world\nbaz qux\n");

        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn test_patch_not_found() {
        let path = "/tmp/arli-patch-test2.txt";
        std::fs::write(path, "hello world\n").unwrap();

        let tool = PatchTool;
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": path,
                    "old_string": "nonexistent",
                    "new_string": "replacement"
                })
                .to_string(),
            )
            .await;

        assert!(!result.success);
        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn test_patch_replace_all() {
        let path = "/tmp/arli-patch-test3.txt";
        std::fs::write(path, "foo foo foo\nbar foo\n").unwrap();

        let tool = PatchTool;
        let result = tool
            .execute(
                &serde_json::json!({
                    "path": path,
                    "old_string": "foo",
                    "new_string": "X",
                    "replace_all": true
                })
                .to_string(),
            )
            .await;

        assert!(result.success);
        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(content, "X X X\nbar X\n");

        std::fs::remove_file(path).ok();
    }
}
