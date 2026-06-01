//! Context management: token counting, pressure detection, and compaction.
//!
//! Uses tiktoken-rs for accurate token counting (not char/4 heuristic).
//! Supports multiple models by mapping model names to tiktoken encodings.

use crate::providers::ChatMessage;
use tiktoken_rs::{cl100k_base, o200k_base, p50k_base, CoreBPE};
use tracing::debug;

/// Maps a provider+model to the appropriate tiktoken encoding.
pub struct TokenCounter {
    bpe: CoreBPE,
    context_window: usize,
}

impl TokenCounter {
    /// Create a token counter for the given model.
    ///
    /// Falls back to cl100k_base for unknown models (reasonable default).
    pub fn new(model: &str, context_window: usize) -> Self {
        let bpe = if model.contains("gpt-4o") || model.contains("gpt-4-turbo") {
            o200k_base().expect("o200k_base tiktoken encoding to load")
        } else if model.contains("gpt-4")
            || model.contains("gpt-3.5")
            || model.contains("deepseek")
            || model.contains("text-embedding")
        {
            cl100k_base().expect("cl100k_base tiktoken encoding to load")
        } else if model.contains("davinci") || model.contains("babbage") {
            p50k_base().expect("p50k_base tiktoken encoding to load")
        } else {
            // Default: cl100k works for most modern models
            cl100k_base().expect("cl100k_base tiktoken encoding to load")
        };

        debug!(
            "TokenCounter init: model={}, context_window={}",
            model, context_window
        );

        Self {
            bpe,
            context_window,
        }
    }

    /// Count tokens in a string.
    pub fn count(&self, text: &str) -> usize {
        self.bpe.encode_ordinary(text).len()
    }

    /// Count tokens in a full ChatMessage.
    pub fn count_message(&self, msg: &ChatMessage) -> usize {
        let mut tokens = 4; // message overhead (role + formatting)

        if let Some(ref content) = msg.content {
            tokens += self.count(content);
        }

        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                tokens += self.count(&tc.function.name);
                tokens += self.count(&tc.function.arguments);
                tokens += 10; // JSON structure overhead
            }
        }

        tokens
    }

    /// Count total tokens across all messages.
    pub fn count_messages(&self, messages: &[ChatMessage]) -> usize {
        let base: usize = messages.iter().map(|m| self.count_message(m)).sum();
        base + 3 // conversation formatting overhead
    }

    /// The context window size for this model.
    pub fn context_window(&self) -> usize {
        self.context_window
    }

    /// What fraction of the context window is used (0.0 to 1.0+).
    pub fn pressure(&self, messages: &[ChatMessage]) -> f64 {
        let used = self.count_messages(messages) as f64;
        used / self.context_window as f64
    }

    /// How many tokens remain.
    pub fn remaining(&self, messages: &[ChatMessage]) -> usize {
        let used = self.count_messages(messages);
        self.context_window.saturating_sub(used)
    }
}

/// Context pressure level — determines what action to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureLevel {
    /// < 50% — plenty of room
    Normal,
    /// 50-75% — warning, but no action needed
    Elevated,
    /// 75-90% — should compact soon
    High,
    /// > 90% — must compact now
    Critical,
}

impl PressureLevel {
    pub fn from_pressure(p: f64) -> Self {
        if p >= 0.90 {
            PressureLevel::Critical
        } else if p >= 0.75 {
            PressureLevel::High
        } else if p >= 0.50 {
            PressureLevel::Elevated
        } else {
            PressureLevel::Normal
        }
    }
}

/// Compaction result: old messages summarized into a single system message.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The summary text to prepend
    pub summary: String,
    /// How many messages were compacted
    pub compacted_count: usize,
    /// Tokens before compaction
    pub tokens_before: usize,
    /// Tokens after compaction
    pub tokens_after: usize,
    /// The new compacted message list (head + summary + tail)
    pub messages: Vec<ChatMessage>,
}

/// Compact messages by summarizing older turns.
///
/// Strategy (same as Hermes, Pi, OpenClaw):
/// 1. Keep the most recent messages (protect_last_n)
/// 2. Keep the first message (system prompt)
/// 3. Summarize everything in between via an LLM call
///
/// This function calculates WHAT to compact. The actual summarization
/// is done by the agent calling an LLM (to avoid circular dependency).
pub fn plan_compaction(
    messages: &[ChatMessage],
    protect_last_n: usize,
    protect_first_n: usize,
) -> (Vec<usize>, Vec<usize>) {
    if messages.len() <= protect_last_n + protect_first_n {
        return (vec![], vec![]);
    }

    let total = messages.len();
    let keep_head: Vec<usize> = (0..protect_first_n.min(total)).collect();
    let _keep_tail: Vec<usize> = (total.saturating_sub(protect_last_n)..total).collect();

    // Messages to summarize: everything between keep_head and keep_tail
    let to_summarize: Vec<usize> =
        (protect_first_n..total.saturating_sub(protect_last_n)).collect();

    (keep_head, to_summarize)
}

/// Format the compacted summary as a system message for continuation.
pub fn format_compaction_summary(summary: &str) -> ChatMessage {
    ChatMessage::system(format!(
        "[This conversation was continued from a previous session. \
         Summary of what happened before:]\n\n{}",
        summary
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Role;

    #[test]
    fn test_plan_compaction_small_conversation() {
        let msgs: Vec<ChatMessage> = (0..5)
            .map(|i| ChatMessage {
                role: Role::User,
                content: Some(format!("msg {}", i)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            })
            .collect();

        let (head, mid) = plan_compaction(&msgs, 3, 1);
        // With 5 messages: keep [0], keep [2,3,4], summarize [1]
        // Actually: protect_first_n=1 keeps [0], protect_last_n=3 keeps [2,3,4]
        // So mid should be [1]
        assert_eq!(head, vec![0]);
        assert_eq!(mid, vec![1]);
    }

    #[test]
    fn test_plan_compaction_tiny() {
        let msgs: Vec<ChatMessage> = (0..3)
            .map(|i| ChatMessage {
                role: Role::User,
                content: Some(format!("msg {}", i)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            })
            .collect();

        let (head, mid) = plan_compaction(&msgs, 3, 1);
        // 3 messages, protect_first=1 + protect_last=3 = 4 > total=3
        // No compaction possible
        assert!(head.is_empty());
        assert!(mid.is_empty());
    }

    #[test]
    fn test_pressure_levels() {
        assert_eq!(PressureLevel::from_pressure(0.30), PressureLevel::Normal);
        assert_eq!(PressureLevel::from_pressure(0.60), PressureLevel::Elevated);
        assert_eq!(PressureLevel::from_pressure(0.80), PressureLevel::High);
        assert_eq!(PressureLevel::from_pressure(0.95), PressureLevel::Critical);
    }

    #[test]
    fn test_token_counter_basic() {
        let tc = TokenCounter::new("deepseek-chat", 128_000);
        let tokens = tc.count("Hello, world!");
        // "Hello, world!" = ~4 tokens in cl100k
        assert!(tokens >= 3 && tokens <= 6, "got {} tokens", tokens);
    }
}
