//! Browser tool — fetch web pages and extract content.
//!
//! Uses reqwest to fetch URLs and extracts readable text from HTML.
//! Handles truncation for large pages and basic error reporting.

use async_trait::async_trait;
use super::{Tool, ToolOutput};

pub struct BrowserTool;

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str { "browser" }

    fn description(&self) -> &str {
        "Fetch a URL and return its content as text. \
         Use this to read web pages, API responses, or documentation. \
         Handles HTML → text extraction automatically. \
         Use 'mode=raw' to get unprocessed content."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "mode": {
                    "type": "string",
                    "enum": ["text", "raw"],
                    "description": "text = extract readable text from HTML, raw = unprocessed response",
                    "default": "text"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutput::error(&format!("Invalid JSON: {}", e)),
        };

        let url = match args["url"].as_str() {
            Some(u) => u,
            None => return ToolOutput::error("Missing 'url' parameter"),
        };

        let mode = args["mode"].as_str().unwrap_or("text");

        // Fetch
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("ARLI/0.1 (agent harness)")
            .build()
            .unwrap_or_default();

        let response = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(&format!("Failed to fetch '{}': {}", url, e)),
        };

        let status = response.status();
        let content_type = response.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string(); // Clone to own the string

        let body = match response.text().await {
            Ok(b) => b,
            Err(e) => return ToolOutput::error(&format!("Failed to read body: {}", e)),
        };

        let output = if mode == "raw" {
            body
        } else if content_type.contains("text/html") || content_type.contains("application/xhtml") {
            html_to_text(&body)
        } else if content_type.contains("application/json") {
            // Pretty-print JSON
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                serde_json::to_string_pretty(&v).unwrap_or(body)
            } else {
                body
            }
        } else {
            body
        };

        // Truncate
        let truncated = if output.len() > 20000 {
            format!("{}...\n\n[Truncated at 20000 chars. {} total chars, HTTP {}]",
                &output[..20000], output.len(), status.as_u16())
        } else {
            format!("{}\n\n[HTTP {}, {} bytes]", output, status.as_u16(), output.len())
        };

        ToolOutput {
            success: status.is_success(),
            content: truncated,
            error: if status.is_success() { None } else { Some(format!("HTTP {}", status.as_u16())) },
        }
    }
}

/// Very basic HTML to text extraction.
fn html_to_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_buf = String::new();

    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_buf.clear();
            }
            '>' => {
                in_tag = false;
                let tag_lower = tag_buf.to_lowercase();
                if tag_lower.starts_with("script") || tag_lower.starts_with("/script") {
                    in_script = tag_lower.starts_with("script");
                }
                if tag_lower.starts_with("style") || tag_lower.starts_with("/style") {
                    in_style = tag_lower.starts_with("style");
                }
                // Block-level tags: insert newline
                if tag_lower.starts_with("br") || tag_lower.starts_with("p") || tag_lower.starts_with("/p")
                    || tag_lower.starts_with("div") || tag_lower.starts_with("h1") || tag_lower.starts_with("h2")
                    || tag_lower.starts_with("li") || tag_lower.starts_with("tr")
                {
                    text.push('\n');
                }
                tag_buf.clear();
            }
            c if in_tag => {
                tag_buf.push(c);
            }
            _c if in_script || in_style => {
                // Skip script/style content
            }
            c => {
                text.push(c);
            }
        }
    }

    // Clean up: collapse whitespace, remove blank lines
    let mut clean = String::new();
    let mut last_was_newline = false;
    let mut last_was_space = false;

    for ch in text.chars() {
        if ch == '\n' {
            if !last_was_newline {
                clean.push('\n');
                last_was_newline = true;
                last_was_space = false;
            }
        } else if ch.is_whitespace() {
            if !last_was_space && !last_was_newline {
                clean.push(' ');
                last_was_space = true;
            }
        } else {
            clean.push(ch);
            last_was_newline = false;
            last_was_space = false;
        }
    }

    clean.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_text() {
        let html = "<html><body><h1>Title</h1><p>Hello <b>world</b></p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Title"));
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
    }
}
