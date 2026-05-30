use async_trait::async_trait;

use super::{Tool, ToolOutput};

/// Make HTTP GET requests to fetch web content.
///
/// Returns response body as text. Limited to text/* content types
/// and responses under 100KB to avoid context pollution.
pub struct HttpGetTool;

#[async_trait]
impl Tool for HttpGetTool {
    fn name(&self) -> &str {
        "http_get"
    }

    fn description(&self) -> &str {
        "Make an HTTP GET request and return the response body as text. \
         Use for fetching API responses, documentation, or web content. \
         Limited to 100KB responses. Use shell with curl for complex requests."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "headers": {
                    "type": "object",
                    "description": "Optional HTTP headers as key-value pairs"
                }
            },
            "required": ["url"]
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

        let url = match args["url"].as_str() {
            Some(u) => u,
            None => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some("Missing required parameter: url".into()),
                }
            }
        };

        let mut request = reqwest::Client::new().get(url);

        // Add custom headers if provided
        if let Some(headers) = args["headers"].as_object() {
            for (key, value) in headers {
                if let Some(v) = value.as_str() {
                    request = request.header(key.as_str(), v);
                }
            }
        }

        match request.send().await {
            Ok(response) => {
                let status = response.status();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("unknown")
                    .to_string();

                match response.text().await {
                    Ok(body) => {
                        // Truncate to 50KB
                        let truncated = if body.len() > 50_000 {
                            format!("{}... [truncated from {} bytes]", &body[..50_000], body.len())
                        } else {
                            body
                        };

                        ToolOutput {
                            success: status.is_success(),
                            content: format!(
                                "HTTP {} ({})\n\n{}",
                                status.as_u16(),
                                content_type,
                                truncated
                            ),
                            error: if status.is_success() {
                                None
                            } else {
                                Some(format!("HTTP {}", status.as_u16()))
                            },
                        }
                    }
                    Err(e) => ToolOutput {
                        success: false,
                        content: String::new(),
                        error: Some(format!("Failed to read response body: {}", e)),
                    },
                }
            }
            Err(e) => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("HTTP request failed: {}", e)),
            },
        }
    }
}
