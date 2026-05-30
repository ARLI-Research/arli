//! OpenRouter provider — access 200+ models through a single API.
//!
//! OpenRouter is an OpenAI-compatible API gateway that routes to
//! dozens of providers (OpenAI, Anthropic, Google, Meta, Mistral, etc.)
//!
//! Usage: set OPENROUTER_API_KEY, model = "openai/gpt-4o" or "anthropic/claude-3.5-sonnet"

use async_trait::async_trait;
use crate::context::TokenCounter;
use crate::error::Result;
use crate::providers::{
    ChatMessage, FunctionCall, LlmResponse, LlmResponseContent,
    Provider, ToolCall, ToolSchema,
};

/// OpenRouter provider — OpenAI-compatible API.
pub struct OpenRouterProvider {
    api_key: String,
    model: String,
    base_url: String,
    token_counter: TokenCounter,
}

impl OpenRouterProvider {
    pub fn new(api_key: String, model: String) -> Self {
        let context_window = if model.contains("claude") {
            200_000
        } else if model.contains("gpt-4") || model.contains("o1") || model.contains("o3") {
            128_000
        } else if model.contains("gemini") {
            1_000_000
        } else {
            128_000 // default
        };

        Self {
            api_key,
            model: model.clone(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            token_counter: TokenCounter::new(&model, context_window),
        }
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
    ) -> Result<LlmResponse> {
        let client = reqwest::Client::new();

        // Convert messages to OpenAI format
        let openai_msgs: Vec<serde_json::Value> = messages.iter().map(|msg| {
            let mut m = serde_json::json!({
                "role": format!("{:?}", msg.role).to_lowercase(),
            });

            if let Some(ref content) = msg.content {
                m["content"] = serde_json::json!(content);
            }

            if let Some(ref tool_calls) = msg.tool_calls {
                m["tool_calls"] = serde_json::json!(tool_calls);
            }

            if let Some(ref tool_call_id) = msg.tool_call_id {
                m["tool_call_id"] = serde_json::json!(tool_call_id);
            }

            m
        }).collect();

        // Convert tools to OpenAI format
        let openai_tools: Vec<serde_json::Value> = tools.iter().map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": t.function.parameters
                }
            })
        }).collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": openai_msgs,
        });

        if !openai_tools.is_empty() {
            body["tools"] = serde_json::json!(openai_tools);
        }

        let response = client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://github.com/arliwork/arli")
            .header("X-Title", "ARLI Agent Harness")
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::Error::Provider(format!("HTTP error: {}", e)))?;

        let status = response.status();
        let resp_body: serde_json::Value = response.json().await
            .map_err(|e| crate::error::Error::Provider(format!("JSON parse error: {}", e)))?;

        if !status.is_success() {
            let err_msg = resp_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return crate::error::Result::Err(
                crate::error::Error::Provider(format!("OpenRouter API error ({}): {}", status.as_u16(), err_msg))
            );
        }

        let choice = &resp_body["choices"][0];
        let message = &choice["message"];

        let usage = resp_body["usage"].as_object().map(|u| {
            crate::providers::Usage {
                prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                completion_tokens: u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            }
        });

        // Check for tool calls
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            let calls: Vec<ToolCall> = tool_calls.iter().map(|tc| {
                ToolCall {
                    id: tc["id"].as_str().unwrap_or("unknown").to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: tc["function"]["name"].as_str().unwrap_or("").to_string(),
                        arguments: tc["function"]["arguments"].as_str().unwrap_or("{}").to_string(),
                    },
                }
            }).collect();

            Ok(LlmResponse {
                content: LlmResponseContent::ToolCalls {
                    content: message["content"].as_str().map(|s| s.to_string()),
                    tool_calls: calls,
                },
                reasoning: None,
                usage,
            })
        } else {
            Ok(LlmResponse {
                content: LlmResponseContent::Text {
                    content: message["content"].as_str().unwrap_or("").to_string(),
                },
                reasoning: None,
                usage,
            })
        }
    }

    fn token_counter(&self) -> &TokenCounter { &self.token_counter }
    fn name(&self) -> &str { "openrouter" }
    fn model(&self) -> &str { &self.model }
    fn supports_streaming(&self) -> bool { true }
}
