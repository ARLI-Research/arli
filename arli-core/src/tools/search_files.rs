use async_trait::async_trait;

use super::{Tool, ToolOutput};

/// Search file contents or find files by name — in-process, no shell-out.
///
/// Content search: regex inside files via ripgrep crates (grep-regex + grep-searcher).
/// File search: glob-based file discovery via `ignore` crate.
///
/// No fork/exec. Works cross-platform without requiring `rg` or `find` installed.
pub struct SearchFilesTool;

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
    }

    fn description(&self) -> &str {
        "Search inside files (content) or find files by name (glob). \
         Content search: regex pattern in files, returns matching lines with line numbers. \
         File search: glob pattern to find files by name. \
         In-process — no shell-out. Use this instead of grep/find/ls in shell."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern for content search, or glob for file search"
                },
                "target": {
                    "type": "string",
                    "enum": ["content", "files"],
                    "description": "'content' searches inside files, 'files' finds files by name",
                    "default": "content"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "file_glob": {
                    "type": "string",
                    "description": "Filter files by glob pattern (e.g. '*.py')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default: 30)",
                    "default": 30
                }
            },
            "required": ["pattern"]
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

        let pattern = match args["pattern"].as_str() {
            Some(p) => p,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: pattern".into()),
                }
            }
        };

        let target = args["target"].as_str().unwrap_or("content");
        let search_path = args["path"].as_str().unwrap_or(".");
        let file_glob = args["file_glob"].as_str();
        let limit = args["limit"].as_u64().unwrap_or(30) as usize;

        match target {
            "files" => {
                // Find files by glob pattern — in-process via ignore crate
                match crate::native_search::find_files(pattern, search_path, file_glob, limit) {
                    Ok(files) => {
                        if files.is_empty() {
                            return ToolOutput {
                                success: true,
                                content: format!(
                                    "No files matching '{}' found in {}",
                                    pattern, search_path
                                ),
                                error: None,
                            };
                        }

                        let total = files.len();
                        let mut result =
                            format!("Found {} file(s) matching '{}':\n", total, pattern);
                        for f in &files {
                            result.push_str(&format!("  {}\n", f));
                        }

                        ToolOutput {
                            success: true,
                            content: result,
                            error: None,
                        }
                    }
                    Err(e) => ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some(format!("find failed: {}", e)),
                    },
                }
            }
            _ => {
                // Content search — in-process via grep-regex + grep-searcher
                match crate::native_search::grep_content(pattern, search_path, file_glob, limit) {
                    Ok(matches) => {
                        if matches.is_empty() {
                            return ToolOutput {
                                success: true,
                                content: format!("No matches found for '{}'", pattern),
                                error: None,
                            };
                        }

                        let total = matches.len();
                        let mut result = String::new();
                        for m in &matches {
                            result.push_str(&format!("{}:{}: {}\n", m.file, m.line, m.content));
                        }
                        if total >= limit {
                            result.push_str(&format!(
                                "... truncated {} more results. Narrow your search.\n",
                                total.saturating_sub(limit)
                            ));
                        }

                        ToolOutput {
                            success: true,
                            content: result,
                            error: None,
                        }
                    }
                    Err(e) => ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some(format!("search failed: {}", e)),
                    },
                }
            }
        }
    }
}
