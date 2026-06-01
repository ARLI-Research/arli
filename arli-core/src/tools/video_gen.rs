//! Video generation tool stub.
//!
//! Supports FAL.ai Kling and Runway Gen-3/Gen-4 video generation.
//! This is a premium feature stub — the tool returns an informative
//! message directing users to set up the required API keys.

use async_trait::async_trait;
use super::{Tool, ToolOutput};

pub struct VideoGenTool;

#[async_trait]
impl Tool for VideoGenTool {
    fn name(&self) -> &str {
        "video_generate"
    }

    fn description(&self) -> &str {
        "Generate a video from a text prompt. \
         Supports Kling (FAL.ai) and Runway (Gen-3/Gen-4) providers. \
         This is a premium feature requiring API keys."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Text prompt describing the video to generate"
                },
                "provider": {
                    "type": "string",
                    "enum": ["kling", "runway"],
                    "description": "Video generation provider: 'kling' (FAL.ai Kling) or 'runway' (Runway Gen-3/Gen-4)",
                    "default": "kling"
                },
                "output_path": {
                    "type": "string",
                    "description": "File path to save the generated video (e.g., 'output.mp4')"
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, _arguments: &str) -> ToolOutput {
        ToolOutput {
            success: true,
            content: "Video generation is a premium feature. Use FAL.ai Kling or Runway API.".to_string(),
            error: None,
        }
    }
}
