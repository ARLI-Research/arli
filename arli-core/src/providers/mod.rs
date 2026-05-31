pub mod traits;
pub mod openai;
pub mod anthropic;
pub mod openrouter;

pub use traits::Provider;
pub use openai::OpenAIProvider;
pub use anthropic::AnthropicProvider;
pub use openrouter::OpenRouterProvider;

use crate::config::Config;
use crate::error::Result;

/// Create the right provider based on config.
/// Returns a boxed Provider trait object.
pub fn create_provider(config: &Config) -> Result<Box<dyn Provider>> {
    match config.provider.name.as_str() {
        "openai" => {
            let base_url = config.provider.base_url.clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            Ok(Box::new(OpenAIProvider::new(
                config.provider.api_key.clone(),
                config.model.clone(),
                Some(base_url),
            )))
        }
        "deepseek" => {
            let base_url = config.provider.base_url.clone()
                .unwrap_or_else(|| "https://api.deepseek.com/v1".to_string());
            Ok(Box::new(OpenAIProvider::new(
                config.provider.api_key.clone(),
                config.model.clone(),
                Some(base_url),
            )))
        }
        "openrouter" => Ok(Box::new(OpenRouterProvider::new(
            config.provider.api_key.clone(),
            config.model.clone(),
        ))),
        "anthropic" => Ok(Box::new(AnthropicProvider::new(
            config.provider.api_key.clone(),
            config.model.clone(),
        ))),
        unknown => Err(crate::error::Error::Config(format!(
            "Unknown provider '{}'. Supported: openai, deepseek, openrouter, anthropic",
            unknown
        ))),
    }
}

use serde::{Deserialize, Serialize};

/// Message role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LlmResponseContent {
    Text {
        content: String,
    },
    ToolCalls {
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
}

pub struct LlmResponse {
    pub content: LlmResponseContent,
    pub reasoning: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub function: FunctionSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A single delta chunk from a streaming response.
#[derive(Debug, Clone)]
pub struct StreamDelta {
    /// Text content for this delta (None if this is a tool call update)
    pub content: Option<String>,
    /// Tool calls (only populated when streaming finishes or on tool call)
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Whether the stream is complete
    pub done: bool,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
}

impl ToolResult {
    pub fn new(tool_call_id: String, content: String) -> Self {
        Self {
            tool_call_id,
            content,
            role: "tool".to_string(),
        }
    }

    pub fn as_message(&self) -> ChatMessage {
        ChatMessage {
            role: Role::Tool,
            content: Some(self.content.clone()),
            tool_calls: None,
            tool_call_id: Some(self.tool_call_id.clone()),
            name: None,
        }
    }
}
