//! Vision tool — download and analyze images.
//!
//! Downloads images from URLs, extracts metadata (dimensions, format, size),
//! and provides the image data for further processing by vision-capable LLMs.

use async_trait::async_trait;
use super::{Tool, ToolOutput};

pub struct VisionTool;

#[async_trait]
impl Tool for VisionTool {
    fn name(&self) -> &str { "vision" }

    fn description(&self) -> &str {
        "Download and analyze an image from a URL. Returns dimensions, format, \
         file size, and saves to a local file. Use this to inspect images, \
         screenshots, charts, or photos. The image can then be passed to \
         vision-capable models for deeper analysis."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL of the image to download and analyze"
                },
                "save_path": {
                    "type": "string",
                    "description": "Optional local path to save the image. Defaults to a temp file."
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

        // Download the image
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("ARLI/0.2 (agent harness)")
            .build()
            .unwrap_or_default();

        let response = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(&format!("Failed to download image: {}", e)),
        };

        if !response.status().is_success() {
            return ToolOutput::error(&format!("HTTP {} downloading image", response.status().as_u16()));
        }

        let content_type = response.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string(); // Clone to own before moving response

        // Check it's an image
        if !content_type.starts_with("image/") {
            return ToolOutput::error(&format!(
                "URL does not point to an image (Content-Type: {}). Use browser tool for web pages.",
                content_type
            ));
        }

        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => return ToolOutput::error(&format!("Failed to read image data: {}", e)),
        };

        let file_size = bytes.len();

        // Parse image metadata
        let img = match image::load_from_memory(&bytes) {
            Ok(img) => img,
            Err(e) => {
                // Try image::io::Reader as fallback
                match image::io::Reader::new(std::io::Cursor::new(&bytes))
                    .with_guessed_format()
                    .ok()
                    .and_then(|r| r.decode().ok())
                {
                    Some(img) => img,
                    None => return ToolOutput::error(&format!(
                        "Cannot decode image: {}. Content-Type was '{}', {} bytes",
                        e, content_type, file_size
                    )),
                }
            }
        };

        let dimensions = (img.width(), img.height());
        let color_type = format!("{:?}", img.color());

        // Save to file
        let save_path = if let Some(path) = args["save_path"].as_str() {
            path.to_string()
        } else {
            // Generate temp path from URL
            let ext = content_type.strip_prefix("image/").unwrap_or("png");
            let safe_name = url
                .split('/')
                .last()
                .unwrap_or("image")
                .split('?')
                .next()
                .unwrap_or("image");
            let temp_dir = std::env::temp_dir();
            format!("{}/{}.{}", temp_dir.display(), safe_name, ext)
        };

        // Ensure directory exists
        if let Some(parent) = std::path::Path::new(&save_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }

        match img.save(&save_path) {
            Ok(_) => {
                let format = content_type.strip_prefix("image/").unwrap_or("unknown");
                ToolOutput {
                    success: true,
                    content: format!(
                        "Image downloaded successfully:\n\
                         \n  URL: {}\n\
                         \n  Saved: {}\n\
                         \n  Format: {} ({})\n\
                         \n  Dimensions: {}×{}px\n\
                         \n  Color: {}\n\
                         \n  Size: {} bytes ({:.1} KB)",
                        url,
                        save_path,
                        format,
                        content_type,
                        dimensions.0,
                        dimensions.1,
                        color_type,
                        file_size,
                        file_size as f64 / 1024.0,
                    ),
                    error: None,
                }
            }
            Err(e) => ToolOutput::error(&format!("Failed to save image: {}", e)),
        }
    }
}
