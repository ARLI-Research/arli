use crate::context::TokenCounter;
use crate::error::Result;
use async_trait::async_trait;

use super::{ChatMessage, LlmResponse, LlmResponseContent, StreamDelta, ToolSchema};

/// Provider trait — implemented by every LLM backend.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Send a chat completion request.
    async fn chat(&self, messages: &[ChatMessage], tools: &[ToolSchema]) -> Result<LlmResponse>;

    /// Send a streaming chat completion request.
    /// Returns a receiver for delta chunks.
    /// Default implementation fall back to non-streaming chat.
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
    ) -> Result<tokio::sync::mpsc::Receiver<StreamDelta>> {
        // Default: non-streaming fallback
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let response = self.chat(messages, tools).await?;
        match response.content {
            LlmResponseContent::Text { content } => {
                let _ = tx
                    .send(StreamDelta {
                        content: Some(content),
                        tool_calls: None,
                        done: true,
                    })
                    .await;
            }
            LlmResponseContent::ToolCalls {
                content,
                tool_calls,
            } => {
                if let Some(text) = content {
                    let _ = tx
                        .send(StreamDelta {
                            content: Some(text),
                            tool_calls: None,
                            done: false,
                        })
                        .await;
                }
                let _ = tx
                    .send(StreamDelta {
                        content: None,
                        tool_calls: Some(tool_calls),
                        done: true,
                    })
                    .await;
            }
        }
        Ok(rx)
    }

    /// Get the token counter for this provider's model.
    fn token_counter(&self) -> &TokenCounter;

    /// Count tokens in a string using the model's tokenizer.
    fn count_tokens(&self, text: &str) -> usize {
        self.token_counter().count(text)
    }

    /// Count tokens across all messages.
    fn count_message_tokens(&self, messages: &[ChatMessage]) -> usize {
        self.token_counter().count_messages(messages)
    }

    /// Context window size.
    fn context_window(&self) -> usize {
        self.token_counter().context_window()
    }

    /// Fraction of context window used (0.0 to 1.0+)
    fn pressure(&self, messages: &[ChatMessage]) -> f64 {
        self.token_counter().pressure(messages)
    }

    /// Whether this provider supports streaming.
    fn supports_streaming(&self) -> bool {
        true
    }

    /// Human-readable name.
    fn name(&self) -> &str;

    /// Model identifier.
    fn model(&self) -> &str;
}
