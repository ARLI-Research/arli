use async_trait::async_trait;

use super::{Tool, ToolOutput};

/// Search file contents or find files by name using ripgrep.
///
/// Content search: regex search inside files with line numbers.
/// File search: find files by glob pattern.
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
         Use this instead of grep/find/ls in shell."
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
                // Find files by name pattern
                let mut cmd = std::process::Command::new("find");
                cmd.arg(search_path)
                    .arg("-name")
                    .arg(pattern)
                    .arg("-type")
                    .arg("f")
                    .arg("-maxdepth")
                    .arg("5");

                if let Some(glob) = file_glob {
                    cmd.arg("-name").arg(glob);
                }

                match cmd.output() {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
                        let total = lines.len();
                        let shown: Vec<&str> = lines.into_iter().take(limit).collect();

                        if shown.is_empty() {
                            return ToolOutput {
                                success: true,
                                content: format!(
                                    "No files matching '{}' found in {}",
                                    pattern, search_path
                                ),
                                error: None,
                            };
                        }

                        let mut result =
                            format!("Found {} file(s) matching '{}':\n", total, pattern);
                        for f in shown {
                            result.push_str(&format!("  {}\n", f));
                        }
                        if total > limit {
                            result.push_str(&format!("... and {} more\n", total - limit));
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
                // Content search using ripgrep
                let mut cmd = std::process::Command::new("rg");
                cmd.arg("--line-number")
                    .arg("--no-heading")
                    .arg("--color")
                    .arg("never")
                    .arg("-n"); // line numbers

                if let Some(glob) = file_glob {
                    cmd.arg("--glob").arg(glob);
                }

                cmd.arg(pattern).arg(search_path);

                // Limit results
                let max_results = (limit * 2).to_string();
                cmd.arg("-m").arg(&max_results);

                match cmd.output() {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);

                        if output.status.code() == Some(1) {
                            // ripgrep returns 1 for no matches
                            return ToolOutput {
                                success: true,
                                content: format!("No matches found for '{}'", pattern),
                                error: None,
                            };
                        }

                        if !output.status.success() {
                            return ToolOutput {
                                success: false,
                                content: String::new(),
                                error: Some(format!("ripgrep error: {}", stderr)),
                            };
                        }

                        let lines: Vec<&str> = stdout.lines().collect();
                        let total = lines.len();
                        let shown = if total > limit {
                            &lines[..limit]
                        } else {
                            &lines[..]
                        };

                        if shown.is_empty() {
                            return ToolOutput {
                                success: true,
                                content: format!("No matches for '{}'", pattern),
                                error: None,
                            };
                        }

                        let mut result = String::new();
                        for line in shown {
                            result.push_str(line);
                            result.push('\n');
                        }
                        if total > limit {
                            result.push_str(&format!(
                                "... truncated {} more results. Narrow your search.\n",
                                total - limit
                            ));
                        }

                        ToolOutput {
                            success: true,
                            content: result,
                            error: None,
                        }
                    }
                    Err(e) => {
                        // ripgrep not installed — fall back to grep
                        let mut cmd = std::process::Command::new("grep");
                        cmd.arg("-rn")
                            .arg("--color=never")
                            .arg(pattern)
                            .arg(search_path);

                        if let Some(glob) = file_glob {
                            cmd.arg("--include").arg(glob);
                        }

                        match cmd.output() {
                            Ok(output) => {
                                let stdout = String::from_utf8_lossy(&output.stdout);
                                if stdout.trim().is_empty() {
                                    return ToolOutput {
                                        success: true,
                                        content: format!(
                                            "No matches for '{}' (rg not available, used grep)",
                                            pattern
                                        ),
                                        error: None,
                                    };
                                }
                                let lines: Vec<&str> = stdout.lines().take(limit).collect();
                                ToolOutput {
                                    success: true,
                                    content: lines.join("\n"),
                                    error: Some(format!(
                                        "ripgrep unavailable ({}), fell back to grep",
                                        e
                                    )),
                                }
                            }
                            Err(e2) => ToolOutput {
                                success: false,
                                content: String::new(),
                                error: Some(format!("Both rg and grep failed: {} / {}", e, e2)),
                            },
                        }
                    }
                }
            }
        }
    }
}
