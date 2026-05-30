use async_trait::async_trait;
use std::path::PathBuf;

use super::{Tool, ToolOutput};

/// Search past sessions using FTS5.
///
/// Opens a temporary connection for each search to work around
/// rusqlite's RefCell-based Connection not being Send.
pub struct SessionSearchTool {
    db_path: PathBuf,
}

impl SessionSearchTool {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search past conversation sessions for relevant information. \
         Use this to find past decisions, code changes, errors, or context \
         that is no longer in the active conversation window. \
         Returns matching messages found via full-text search."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query — keywords to find in past sessions"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 5)",
                    "default": 5
                }
            },
            "required": ["query"]
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

        let query = match args["query"].as_str() {
            Some(q) => q,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: query".into()),
                }
            }
        };

        let limit = args["limit"].as_u64().unwrap_or(5) as usize;

        // Open a fresh connection for this search
        match crate::session::SessionStore::open(self.db_path.clone()) {
            Ok(store) => match store.search(query, limit, None) {
                Ok(results) => {
                    if results.is_empty() {
                        return ToolOutput {
                            success: true,
                            content: format!("No results found for '{}' in past sessions.", query),
                            error: None,
                        };
                    }

                    let mut output = format!(
                        "Found {} result(s) for '{}':\n\n",
                        results.len(),
                        query
                    );

                    for (i, r) in results.iter().enumerate() {
                        output.push_str(&format!(
                            "[{}] session={} role={}\n    {}\n\n",
                            i + 1,
                            &r.session_id[..r.session_id.len().min(12)],
                            r.role,
                            r.content
                        ));
                    }

                    ToolOutput {
                        success: true,
                        content: output,
                        error: None,
                    }
                }
                Err(e) => ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Search error: {}", e)),
                },
            },
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Cannot open session DB: {}", e)),
            },
        }
    }
}
