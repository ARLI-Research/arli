//! Web search tool — uses Tavily Search API for real-time web search.
//!
//! Set TAVILY_API_KEY env var to enable. Free tier: 1000 searches/month.
//! Falls back to a helpful error message if not configured.

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolOutput};

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using Tavily Search API. Returns titles, URLs, and snippets. \
         Use this for current information, documentation lookups, price checks, \
         and any query that needs real-time web data. \
         Set TAVILY_API_KEY environment variable to enable."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (natural language or keywords)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (1-10, default 5)",
                    "default": 5
                },
                "search_depth": {
                    "type": "string",
                    "enum": ["basic", "advanced"],
                    "description": "basic (faster) or advanced (more thorough, uses more API credits)",
                    "default": "basic"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Invalid JSON: {}", e)),
                };
            }
        };

        let query = match args["query"].as_str() {
            Some(q) => q,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: query".into()),
                };
            }
        };

        let max_results = args["max_results"].as_u64().unwrap_or(5).min(10) as u32;
        let search_depth = args["search_depth"].as_str().unwrap_or("basic");

        // Read API key from environment
        let api_key = match std::env::var("TAVILY_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                return ToolOutput {
                    success: false,
                    content: "Web search requires TAVILY_API_KEY.\n\
                              Get a free key at https://tavily.com (1000 searches/month)\n\
                              Then: export TAVILY_API_KEY='tvly-...'".into(),
                    error: Some("TAVILY_API_KEY not set".into()),
                };
            }
        };

        // Call Tavily API
        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "api_key": api_key,
            "query": query,
            "max_results": max_results,
            "search_depth": search_depth,
            "include_answer": true,
            "include_raw_content": false,
            "include_images": false,
        });

        match client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                match response.json::<Value>().await {
                    Ok(json) => {
                        if !status.is_success() {
                            let err = json["detail"]["error"]
                                .as_str()
                                .or_else(|| json["error"].as_str())
                                .unwrap_or("Unknown Tavily error");
                            return ToolOutput {
                                success: false,
                                content: String::new(),
                                error: Some(format!("Tavily API error: {}", err)),
                            };
                        }

                        let mut output = String::new();

                        // Add answer if available (Tavily's AI-generated summary)
                        if let Some(answer) = json["answer"].as_str() {
                            if !answer.is_empty() {
                                output.push_str(&format!("Answer: {}\n\n", answer));
                            }
                        }

                        // Add search results
                        if let Some(results) = json["results"].as_array() {
                            if results.is_empty() {
                                output.push_str("No results found.");
                            } else {
                                output.push_str(&format!("Found {} results:\n\n", results.len()));
                                for (i, r) in results.iter().enumerate() {
                                    let title = r["title"].as_str().unwrap_or("No title");
                                    let url = r["url"].as_str().unwrap_or("");
                                    let content = r["content"].as_str().unwrap_or("");

                                    output.push_str(&format!(
                                        "{}. {}\n   {}\n   {}\n\n",
                                        i + 1,
                                        title,
                                        url,
                                        content,
                                    ));
                                }
                            }
                        } else {
                            output.push_str("No results found.");
                        }

                        // Add response time
                        if let Some(time) = json["response_time"].as_f64() {
                            output.push_str(&format!("\nSearch took {:.2}s", time));
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
                        error: Some(format!("Failed to parse Tavily response: {}", e)),
                    },
                }
            }
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Tavily API request failed: {}", e)),
            },
        }
    }
}
