//! Anthropic provider adapter — Claude models via Messages API.
//!
//! API docs: https://docs.anthropic.com/en/api/messages
//! Key differences from OpenAI:
//! - System prompt is a top-level field, not a message
//! - Tools are "tools" not "functions"
//! - Tool results are in "content" blocks with tool_use_id

use async_trait::async_trait;
use crate::context::TokenCounter;
use crate::error::Result;
use crate::providers::{
    ChatMessage, FunctionCall, LlmResponse, LlmResponseContent,
    Provider, Role, ToolCall, ToolSchema,
};

/// Anthropic provider (Claude models).
pub struct AnthropicProvider {
    api_key: String,
    model: String,
    base_url: String,
    token_counter: TokenCounter,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        let context_window = 200_000; // all Claude 3+ models

        Self {
            api_key,
            model: model.clone(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            token_counter: TokenCounter::new(&model, context_window),
        }
    }

    /// Set a custom base URL (e.g., for proxies).
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
    ) -> Result<LlmResponse> {
        let client = reqwest::Client::new();

        // Separate system message from conversation
        let system = messages.first()
            .filter(|m| m.role == Role::System)
            .and_then(|m| m.content.clone());

        let conversation: Vec<&ChatMessage> = if system.is_some() {
            messages.iter().skip(1).collect()
        } else {
            messages.iter().collect()
        };

        // Build Anthropic-format messages
        let mut anthropic_msgs: Vec<serde_json::Value> = Vec::new();

        for msg in &conversation {
            match msg.role {
                Role::User => {
                    anthropic_msgs.push(serde_json::json!({
                        "role": "user",
                        "content": msg.content.as_deref().unwrap_or("")
                    }));
                }
                Role::Assistant => {
                    if let Some(ref tool_calls) = msg.tool_calls {
                        let content: Vec<serde_json::Value> = tool_calls.iter().map(|tc| {
                            serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.function.name,
                                "input": serde_json::from_str::<serde_json::Value>(&tc.function.arguments).unwrap_or(serde_json::json!({}))
                            })
                        }).collect();

                        anthropic_msgs.push(serde_json::json!({
                            "role": "assistant",
                            "content": content
                        }));
                    } else {
                        anthropic_msgs.push(serde_json::json!({
                            "role": "assistant",
                            "content": msg.content.as_deref().unwrap_or("")
                        }));
                    }
                }
                Role::Tool => {
                    anthropic_msgs.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": msg.tool_call_id.as_deref().unwrap_or("unknown"),
                            "content": msg.content.as_deref().unwrap_or("")
                        }]
                    }));
                }
                Role::System => {
                    // Already handled above
                }
            }
        }

        // Build Anthropic-format tools
        let anthropic_tools: Vec<serde_json::Value> = tools.iter().filter_map(|t| {
            if t.function.name.is_empty() { return None; }
            Some(serde_json::json!({
                "name": t.function.name,
                "description": t.function.description,
                "input_schema": t.function.parameters
            }))
        }).collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": 4096,
            "messages": anthropic_msgs,
        });

        if let Some(sys) = &system {
            // Anthropic prompt caching: wrap system in cacheable format
            body["system"] = serde_json::json!([{
                "type": "text",
                "text": sys,
                "cache_control": {"type": "ephemeral"}
            }]);
        }

        // Add cache_control to last 2 messages for prompt caching
        let msg_count = anthropic_msgs.len();
        if msg_count >= 2 {
            if let Some(last) = anthropic_msgs.last_mut() {
                if let Some(obj) = last.as_object_mut() {
                    obj.insert("cache_control".to_string(), serde_json::json!({"type": "ephemeral"}));
                }
            }
            if msg_count >= 3 {
                if let Some(prev) = anthropic_msgs.get_mut(msg_count - 2) {
                    if let Some(obj) = prev.as_object_mut() {
                        obj.insert("cache_control".to_string(), serde_json::json!({"type": "ephemeral"}));
                    }
                }
            }
        }

        if !anthropic_tools.is_empty() {
            body["tools"] = serde_json::json!(anthropic_tools);
        }

        let response = client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
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
                crate::error::Error::Provider(format!("Anthropic API error ({}): {}", status.as_u16(), err_msg))
            );
        }

        // Parse response
        let content_blocks = resp_body["content"].as_array()
            .map(|a| a.to_vec())
            .unwrap_or_default();

        let text_content: Vec<String> = content_blocks.iter()
            .filter_map(|b| b["text"].as_str().map(|t| t.to_string()))
            .collect();

        let tool_uses: Vec<serde_json::Value> = content_blocks.iter()
            .filter(|b| b["type"].as_str() == Some("tool_use"))
            .cloned()
            .collect();

        let usage = resp_body["usage"].as_object().map(|u| {
            crate::providers::Usage {
                prompt_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                completion_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                total_tokens: (u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                    + u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0)) as u32,
            }
        });

        if tool_uses.is_empty() {
            Ok(LlmResponse {
                content: LlmResponseContent::Text {
                    content: text_content.join("\n"),
                },
                reasoning: None,
                usage,
            })
        } else {
            let tool_calls: Vec<ToolCall> = tool_uses.iter().map(|tu| {
                ToolCall {
                    id: tu["id"].as_str().unwrap_or("unknown").to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: tu["name"].as_str().unwrap_or("").to_string(),
                        arguments: tu["input"].to_string(),
                    },
                }
            }).collect();

            Ok(LlmResponse {
                content: LlmResponseContent::ToolCalls {
                    content: if text_content.is_empty() { None } else { Some(text_content.join("\n")) },
                    tool_calls,
                },
                reasoning: None,
                usage,
            })
        }
    }

    fn token_counter(&self) -> &TokenCounter { &self.token_counter }
    fn name(&self) -> &str { "anthropic" }
    fn model(&self) -> &str { &self.model }
    fn supports_streaming(&self) -> bool { true }
}
