use async_trait::async_trait;
use serde_json::Value;

use super::traits::Provider;
use super::{ChatMessage, LlmResponse, LlmResponseContent, ToolCall, ToolSchema};
use crate::context::TokenCounter;
use crate::error::{Error, Result};

/// OpenAI-compatible provider (works with OpenAI, DeepSeek, OpenRouter, etc.)
pub struct OpenAIProvider {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    token_counter: TokenCounter,
    client: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new(api_key: String, model: String, base_url: Option<String>) -> Self {
        let base_url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let token_counter = TokenCounter::new(&model, 128_000);
        Self {
            api_key,
            base_url,
            model,
            token_counter,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    async fn chat(&self, messages: &[ChatMessage], tools: &[ToolSchema]) -> Result<LlmResponse> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::to_value(tools)?;
            body["tool_choice"] = serde_json::json!("auto");
        }

        let token_count = self.token_counter.count_messages(messages);
        let pressure = token_count as f64 / self.token_counter.context_window() as f64;
        tracing::debug!(
            "OpenAI request: {} messages ({} tokens, {:.0}% context) → {}",
            messages.len(),
            token_count,
            pressure * 100.0,
            url
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Provider(format!(
                "HTTP {} from {}: {}",
                status, self.model, text
            )));
        }

        let json: Value = response.json().await?;
        let choice = &json["choices"][0];
        let message = &choice["message"];

        let reasoning = message["reasoning_content"]
            .as_str()
            .or_else(|| message["reasoning"].as_str())
            .map(|s| s.to_string());

        let content = if let Some(tool_calls) = message["tool_calls"].as_array() {
            let calls: Vec<ToolCall> = tool_calls
                .iter()
                .map(|tc| ToolCall {
                    id: tc["id"].as_str().unwrap_or("unknown").to_string(),
                    call_type: tc["type"].as_str().unwrap_or("function").to_string(),
                    function: super::FunctionCall {
                        name: tc["function"]["name"]
                            .as_str()
                            .unwrap_or("unknown")
                            .to_string(),
                        arguments: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or("{}")
                            .to_string(),
                    },
                })
                .collect();

            LlmResponseContent::ToolCalls {
                content: message["content"].as_str().map(|s| s.to_string()),
                tool_calls: calls,
            }
        } else {
            LlmResponseContent::Text {
                content: message["content"]
                    .as_str()
                    .unwrap_or("(no response)")
                    .to_string(),
            }
        };

        let usage = json["usage"].as_object().map(|u| super::Usage {
            prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
            total_tokens: u["total_tokens"].as_u64().unwrap_or(0) as u32,
        });

        Ok(LlmResponse {
            content,
            reasoning,
            usage,
        })
    }

    fn token_counter(&self) -> &TokenCounter {
        &self.token_counter
    }

    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn model(&self) -> &str {
        &self.model
    }
}
