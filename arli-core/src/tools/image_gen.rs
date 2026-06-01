//! Image generation tool — FAL.ai + OpenAI DALL-E.
//!
//! Providers:
//!   1. fal    — FAL.ai flux/dev model (env FAL_KEY, fast, good quality)
//!   2. openai — OpenAI DALL-E 3 (env OPENAI_API_KEY, highest quality)
//!   3. auto   — try FAL first, fall back to OpenAI

use async_trait::async_trait;
use super::{Tool, ToolOutput};

pub struct ImageGenTool;

#[async_trait]
impl Tool for ImageGenTool {
    fn name(&self) -> &str { "image_generate" }

    fn description(&self) -> &str {
        "Generate an image from a text prompt. \
         Providers: fal (FAL.ai flux/dev, fast, default), openai (DALL-E 3), auto (try FAL then OpenAI). \
         Saves the image to output_path."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Image description / prompt"
                },
                "provider": {
                    "type": "string",
                    "enum": ["fal", "openai", "auto"],
                    "description": "Image generation provider (default: fal)"
                },
                "output_path": {
                    "type": "string",
                    "description": "Where to save the image (default: /tmp/arli_image_<timestamp>.png)"
                },
                "size": {
                    "type": "string",
                    "enum": ["1024x1024", "1792x1024", "1024x1792"],
                    "description": "Image size for OpenAI DALL-E (default: 1024x1024)"
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutput::error(&format!("Invalid JSON: {}", e)),
        };

        let prompt = match args["prompt"].as_str() {
            Some(p) => p,
            None => return ToolOutput::error("Missing 'prompt' parameter"),
        };

        let provider = args["provider"].as_str().unwrap_or("fal");
        let size = args["size"].as_str().unwrap_or("1024x1024");

        let output_path = args["output_path"].as_str().map(|s| s.to_string())
            .unwrap_or_else(default_output_path);

        // Ensure output directory exists
        if let Some(parent) = std::path::Path::new(&output_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }

        match provider {
            "fal" => generate_fal(prompt, &output_path).await,
            "openai" => generate_openai(prompt, &output_path, size).await,
            "auto" | _ => {
                let result = generate_fal(prompt, &output_path).await;
                if result.success {
                    return result;
                }
                generate_openai(prompt, &output_path, size).await
            }
        }
    }
}

fn default_output_path() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("/tmp/arli_image_{}.png", ts)
}

// ── FAL.ai (flux/dev) ──

async fn generate_fal(prompt: &str, output_path: &str) -> ToolOutput {
    let api_key = match std::env::var("FAL_KEY") {
        Ok(k) => k,
        Err(_) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some("FAL_KEY not set".into()),
            }
        }
    };

    let body = serde_json::json!({
        "prompt": prompt
    });

    let client = reqwest::Client::new();
    let resp = match client
        .post("https://fal.run/fal-ai/flux/dev")
        .header("Authorization", format!("Key {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("FAL.ai request failed: {}", e)),
            }
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let err_body = resp.text().await.unwrap_or_default();
        return ToolOutput {
            success: false,
            content: String::new(),
            error: Some(format!("FAL.ai HTTP {}: {}", status, err_body)),
        };
    }

    // Parse response for image URL
    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("FAL.ai JSON parse error: {}", e)),
            }
        }
    };

    let image_url = json["images"][0]["url"]
        .as_str()
        .or_else(|| json["image"]["url"].as_str())
        .or_else(|| json["url"].as_str());

    let image_url = match image_url {
        Some(u) => u,
        None => {
            return ToolOutput {
                success: false,
                content: format!("FAL.ai unexpected response: {}", json),
                error: Some("Could not find image URL in FAL.ai response".into()),
            }
        }
    };

    download_and_save(image_url, output_path, "FAL.ai").await
}

// ── OpenAI DALL-E ──

async fn generate_openai(prompt: &str, output_path: &str, size: &str) -> ToolOutput {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some("OPENAI_API_KEY not set".into()),
            }
        }
    };

    let body = serde_json::json!({
        "model": "dall-e-3",
        "prompt": prompt,
        "n": 1,
        "size": size
    });

    let client = reqwest::Client::new();
    let resp = match client
        .post("https://api.openai.com/v1/images/generations")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("OpenAI DALL-E request failed: {}", e)),
            }
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let err_body = resp.text().await.unwrap_or_default();
        return ToolOutput {
            success: false,
            content: String::new(),
            error: Some(format!("OpenAI DALL-E HTTP {}: {}", status, err_body)),
        };
    }

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("OpenAI DALL-E JSON parse error: {}", e)),
            }
        }
    };

    let image_url = json["data"][0]["url"].as_str()
        .or_else(|| json["data"][0]["b64_json"].as_str());

    let image_url = match image_url {
        Some(u) => u,
        None => {
            return ToolOutput {
                success: false,
                content: format!("OpenAI DALL-E unexpected response: {}", json),
                error: Some("Could not find image URL in OpenAI response".into()),
            }
        }
    };

    download_and_save(image_url, output_path, "OpenAI DALL-E").await
}

// ── Shared: download image from URL and save to disk ──

async fn download_and_save(url: &str, output_path: &str, provider_name: &str) -> ToolOutput {
    let client = reqwest::Client::new();
    let resp = match client
        .get(url)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Failed to download image from {}: {}", provider_name, e)),
            }
        }
    };

    if !resp.status().is_success() {
        return ToolOutput {
            success: false,
            content: String::new(),
            error: Some(format!(
                "Download image HTTP {} from {}",
                resp.status(), provider_name
            )),
        };
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Failed to read image bytes from {}: {}", provider_name, e)),
            }
        }
    };

    if bytes.is_empty() {
        return ToolOutput {
            success: false,
            content: String::new(),
            error: Some(format!("{} returned empty image", provider_name)),
        };
    }

    match std::fs::write(output_path, &bytes) {
        Ok(_) => ToolOutput {
            success: true,
            content: format!(
                "Image generated: {}, Size: {} bytes",
                output_path, bytes.len()
            ),
            error: None,
        },
        Err(e) => ToolOutput {
            success: false,
            content: String::new(),
            error: Some(format!("Failed to save image: {}", e)),
        },
    }
}
