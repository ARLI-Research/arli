//! Browser tool — fetch web pages, extract content via CSS selectors.
//!
//! Uses reqwest for HTTP + scraper crate for proper HTML parsing.
//! Supports: navigate, extract (CSS selectors), links, title, text.

use async_trait::async_trait;
use super::{Tool, ToolOutput};

pub struct BrowserTool;

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str { "browser" }

    fn description(&self) -> &str {
        "Fetch a URL and extract structured content from web pages. \
         Actions: navigate (full page text), extract (CSS selector), \
         links (all links with text/URL), title (page title), \
         text (readable text with structure). \
         Use extract with CSS selectors like 'article p', '.content', '#main' \
         to get specific elements."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "action": {
                    "type": "string",
                    "enum": ["navigate", "extract", "links", "title", "text"],
                    "description": "What to do: navigate=full page, extract=CSS selector, links=all links, title=page title, text=readable text",
                    "default": "navigate"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for 'extract' action (e.g., 'article p', '.content', '#main')"
                },
                "max_items": {
                    "type": "integer",
                    "description": "Max results for links/extract (default 20)",
                    "default": 20
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

        let action = args["action"].as_str().unwrap_or("navigate");
        let selector = args["selector"].as_str();
        let max_items = args["max_items"].as_u64().unwrap_or(20).min(100) as usize;

        // Fetch page
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("ARLI/0.2 (agent harness; +https://github.com/ARLI-Research/arli)")
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
            .to_string(); // Clone to own the string before moving response

        let body = match response.text().await {
            Ok(b) => b,
            Err(e) => return ToolOutput::error(&format!("Failed to read body: {}", e)),
        };

        // Handle non-HTML responses
        if !content_type.contains("text/html") && !content_type.contains("application/xhtml") {
            let truncated = truncate(&body, 10000);
            return ToolOutput {
                success: status.is_success(),
                content: format!("[Content-Type: {}] HTTP {}\n\n{}", content_type, status.as_u16(), truncated),
                error: if status.is_success() { None } else { Some(format!("HTTP {}", status.as_u16())) },
            };
        }

        // Parse HTML
        let document = scraper::Html::parse_document(&body);

        match action {
            "navigate" => {
                // Full page: extract all readable text with structure awareness
                let text = extract_structured_text(&document);
                let truncated = truncate(&text, 15000);
                let header = format!("Page: {}\nURL: {}\n", 
                    extract_title(&document), url);
                ToolOutput {
                    success: status.is_success(),
                    content: format!("{}{}\n\n[HTTP {}, {} total chars]",
                        header, truncated, status.as_u16(), text.len()),
                    error: if status.is_success() { None } else { Some(format!("HTTP {}", status.as_u16())) },
                }
            }
            "extract" => {
                let sel_str = match selector {
                    Some(s) => s,
                    None => return ToolOutput::error("Missing 'selector' parameter for extract action"),
                };
                let sel = match scraper::Selector::parse(sel_str) {
                    Ok(s) => s,
                    Err(e) => return ToolOutput::error(&format!("Invalid CSS selector '{}': {:?}", sel_str, e)),
                };

                let elements: Vec<String> = document.select(&sel)
                    .take(max_items)
                    .map(|el| el.text().collect::<Vec<_>>().join(" "))
                    .filter(|t| !t.trim().is_empty())
                    .collect();

                if elements.is_empty() {
                    ToolOutput {
                        success: true,
                        content: format!("No elements matched CSS selector '{}' on {}", sel_str, url),
                        error: None,
                    }
                } else {
                    let count = elements.len();
                    let output = elements.iter()
                        .enumerate()
                        .map(|(i, t)| format!("{}. {}", i + 1, t))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let truncated = truncate(&output, 15000);
                    ToolOutput {
                        success: true,
                        content: format!("Extracted {} elements matching '{}' from {}:\n\n{}{}",
                            count, sel_str, url,
                            truncated,
                            if count > max_items { format!("\n\n[Showing {}/{} results]", max_items, count) } else { String::new() }
                        ),
                        error: None,
                    }
                }
            }
            "links" => {
                let sel = match scraper::Selector::parse("a[href]") {
                    Ok(s) => s,
                    Err(_) => return ToolOutput::error("Internal error: invalid link selector"),
                };

                let mut links: Vec<(String, String)> = Vec::new();
                for el in document.select(&sel).take(max_items) {
                    let text = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                    let href = el.value().attr("href").unwrap_or("").to_string();
                    if !text.is_empty() && !href.is_empty() {
                        links.push((text, href));
                    }
                }

                if links.is_empty() {
                    ToolOutput {
                        success: true,
                        content: format!("No links found on {}", url),
                        error: None,
                    }
                } else {
                    let output = links.iter()
                        .enumerate()
                        .map(|(i, (text, href))| {
                            // Resolve relative URLs
                            let full_url = if href.starts_with("http") {
                                href.clone()
                            } else if href.starts_with('/') {
                                format!("{}{}", get_base_url(url), href)
                            } else if href.starts_with('#') || href.starts_with("javascript:") {
                                href.clone()
                            } else {
                                format!("{}/{}", get_base_url(url), href)
                            };
                            format!("{}. {}\n   {}", i + 1, text, full_url)
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    ToolOutput {
                        success: true,
                        content: format!("Links from {} ({} total):\n\n{}", url, links.len(), truncate(&output, 15000)),
                        error: None,
                    }
                }
            }
            "title" => {
                let title = extract_title(&document);
                let meta_desc = extract_meta(&document, "description");
                let mut output = format!("Title: {}\nURL: {}", title, url);
                if let Some(desc) = meta_desc {
                    output.push_str(&format!("\nDescription: {}", desc));
                }
                ToolOutput {
                    success: true,
                    content: output,
                    error: None,
                }
            }
            "text" => {
                let text = extract_structured_text(&document);
                let truncated = truncate(&text, 15000);
                ToolOutput {
                    success: true,
                    content: format!("{}{}", truncated,
                        if text.len() > 15000 { format!("\n\n[Truncated from {} chars]", text.len()) } else { String::new() }
                    ),
                    error: None,
                }
            }
            unknown => ToolOutput::error(&format!("Unknown action '{}'. Use: navigate, extract, links, title, text", unknown)),
        }
    }
}

/// Extract the page title.
fn extract_title(document: &scraper::Html) -> String {
    if let Ok(sel) = scraper::Selector::parse("title") {
        if let Some(el) = document.select(&sel).next() {
            return el.text().collect::<Vec<_>>().join(" ").trim().to_string();
        }
    }
    "No title".to_string()
}

/// Extract a meta tag value.
fn extract_meta(document: &scraper::Html, name: &str) -> Option<String> {
    let sel = scraper::Selector::parse(&format!("meta[name=\"{}\"]", name)).ok()?;
    document.select(&sel).next()
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.to_string())
}

/// Extract readable text while preserving structure (headings, paragraphs, lists).
fn extract_structured_text(document: &scraper::Html) -> String {
    // Get all text-containing elements in document order
    let body_sel = match scraper::Selector::parse("body *") {
        Ok(s) => s,
        Err(_) => return document.root_element().text().collect::<Vec<_>>().join(" "),
    };

    let mut output = String::new();
    let mut last_was_newline = false;

    for el in document.select(&body_sel) {
        let tag = el.value().name();
        let text: String = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
        if text.is_empty() {
            continue;
        }

        // Add structural breaks based on tag
        let prefix = match tag {
            "h1" | "h2" | "h3" | "h4" => "\n\n## ",
            "p" | "div" | "section" | "article" => "\n\n",
            "li" => "\n- ",
            "br" => "\n",
            "tr" => "\n",
            _ => "",
        };

        let suffix = match tag {
            "h1" | "h2" | "h3" | "h4" => "",
            "p" => "",
            "li" => "",
            _ => "",
        };

        if !prefix.is_empty() {
            output.push_str(prefix);
        } else if last_was_newline {
            output.push(' ');
        }

        output.push_str(&text);
        output.push_str(suffix);

        last_was_newline = suffix.contains('\n') || prefix.contains('\n');
    }

    // Clean up excessive whitespace
    let mut clean = String::new();
    let mut prev_newline = false;
    for ch in output.chars() {
        if ch == '\n' {
            if !prev_newline {
                clean.push('\n');
                prev_newline = true;
            }
        } else if ch.is_whitespace() {
            if !prev_newline {
                clean.push(' ');
            }
        } else {
            clean.push(ch);
            prev_newline = false;
        }
    }

    clean.trim().to_string()
}

/// Get the base URL for resolving relative links.
fn get_base_url(url: &str) -> String {
    if let Some(idx) = url.find("://") {
        let after_proto = &url[idx + 3..];
        if let Some(slash) = after_proto.find('/') {
            url[..idx + 3 + slash].to_string()
        } else {
            url.to_string()
        }
    } else {
        url.to_string()
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        format!("{}...\n[truncated at {} chars, {} total]", &s[..max_chars], max_chars, s.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_title() {
        let html = "<html><head><title>Test Page</title></head><body></body></html>";
        let doc = scraper::Html::parse_document(html);
        assert_eq!(extract_title(&doc), "Test Page");
    }

    #[test]
    fn test_get_base_url() {
        assert_eq!(get_base_url("https://example.com/path/to/page"), "https://example.com");
        assert_eq!(get_base_url("https://example.com"), "https://example.com");
        assert_eq!(get_base_url("https://example.com/"), "https://example.com");
    }

    #[test]
    fn test_extract_structured_text() {
        let html = "<html><body><h1>Title</h1><p>Hello world</p><ul><li>Item 1</li><li>Item 2</li></ul></body></html>";
        let doc = scraper::Html::parse_document(html);
        let text = extract_structured_text(&doc);
        assert!(text.contains("Title"));
        assert!(text.contains("Hello world"));
        assert!(text.contains("Item 1"));
        assert!(text.contains("Item 2"));
    }
}
