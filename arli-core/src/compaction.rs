//! Conversation compaction: summarize old turns to stay within context window.
//!
//! Implementation of the compaction strategy from context.rs.
//! Uses the same LLM provider to generate summaries (aux model support optional).

use crate::context::{plan_compaction, CompactionResult};
use crate::error::Result;
use crate::providers::{ChatMessage, Provider, ToolSchema};
use tracing::{debug, info};

/// Compacts a conversation by summarizing older messages.
///
/// Strategy:
/// 1. Keep the first `protect_first_n` messages (system prompt, etc.)
/// 2. Keep the last `protect_last_n` messages (recent conversation)
/// 3. Summarize everything in between into a single system message
pub struct Compactor {
    protect_last_n: usize,
    protect_first_n: usize,
}

impl Compactor {
    pub fn new(protect_last_n: usize, protect_first_n: usize) -> Self {
        Self {
            protect_last_n,
            protect_first_n,
        }
    }

    /// Check if compaction is needed for these messages.
    pub fn needs_compaction(&self, messages: &[ChatMessage], pressure: f64) -> bool {
        pressure >= 0.85 && messages.len() > self.protect_last_n + self.protect_first_n
    }

    /// Execute compaction: generate a summary and return the new message list.
    ///
    /// Returns None if compaction isn't needed (not enough messages to compact
    /// or the context pressure isn't high enough).
    pub async fn compact(
        &self,
        provider: &dyn Provider,
        messages: &[ChatMessage],
    ) -> Result<Option<CompactionResult>> {
        let (head_indices, middle_indices) =
            plan_compaction(messages, self.protect_last_n, self.protect_first_n);

        if middle_indices.is_empty() {
            debug!(
                "Compaction skipped: nothing to compact (only {} messages)",
                messages.len()
            );
            return Ok(None);
        }

        let tokens_before = provider.count_message_tokens(messages);

        info!(
            "Compacting {} of {} messages ({} tokens before)",
            middle_indices.len(),
            messages.len(),
            tokens_before,
        );

        // Build the compaction prompt
        let to_summarize: Vec<String> = middle_indices
            .iter()
            .map(|&i| {
                let msg = &messages[i];
                let role = format!("{:?}", msg.role);
                let content = msg.content.as_deref().unwrap_or("[tool calls]");
                format!("[{}]: {}", role, content)
            })
            .collect();

        let summary_prompt = format!(
            "Summarize the following conversation excerpt. Focus on:\n\
             - Key decisions made\n\
             - Tools used and their results\n\
             - Errors encountered and how they were resolved\n\
             - Current state of the task\n\
             - Files created or modified\n\n\
             Be concise but complete. This summary will be used to continue the conversation.\n\n\
             CONVERSATION:\n{}",
            to_summarize.join("\n\n")
        );

        // Call the provider (no tools — pure text completion for summary)
        let system_msg = ChatMessage::system(
            "You are a conversation summarizer. Generate a concise, structured summary.",
        );
        let user_msg = ChatMessage::user(summary_prompt);

        let compact_messages = vec![system_msg, user_msg];
        let empty_tools: Vec<ToolSchema> = vec![];

        let response = provider.chat(&compact_messages, &empty_tools).await?;

        let summary = match response.content {
            crate::providers::LlmResponseContent::Text { content } => content,
            crate::providers::LlmResponseContent::ToolCalls { content, .. } => {
                content.unwrap_or_else(|| "Conversation compacted.".to_string())
            }
        };

        info!("Compaction summary generated: {} chars", summary.len());

        // Build new message list: keep head + summary + tail
        let mut new_messages: Vec<ChatMessage> = Vec::new();

        for &i in &head_indices {
            new_messages.push(messages[i].clone());
        }

        // Insert compaction summary as a system message
        let summary_msg = ChatMessage::system(format!(
            "[COMPACTED CONVERSATION — {} messages summarized]\n\n{}",
            middle_indices.len(),
            summary
        ));
        new_messages.push(summary_msg);

        // Keep tail messages
        let total = messages.len();
        let tail_start = total.saturating_sub(self.protect_last_n);
        for msg in messages.iter().skip(tail_start) {
            new_messages.push(msg.clone());
        }

        let tokens_after = provider.count_message_tokens(&new_messages);

        info!(
            "Compaction complete: {} tokens → {} tokens ({:.0}% reduction), {} messages → {} messages",
            tokens_before,
            tokens_after,
            (1.0 - tokens_after as f64 / tokens_before as f64) * 100.0,
            messages.len(),
            new_messages.len(),
        );

        Ok(Some(CompactionResult {
            summary,
            compacted_count: middle_indices.len(),
            tokens_before,
            tokens_after,
            messages: new_messages,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::providers::Role;

    #[test]
    fn test_compactor_needs_compaction() {
        let c = Compactor::new(20, 3);
        let msgs: Vec<ChatMessage> = (0..30)
            .map(|i| ChatMessage {
                role: Role::User,
                content: Some(format!("msg {}", i)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            })
            .collect();

        // High pressure + enough messages → needs compaction
        assert!(c.needs_compaction(&msgs, 0.90));
        // Low pressure → no compaction
        assert!(!c.needs_compaction(&msgs, 0.40));
        // Too few messages → no compaction
        let few: Vec<ChatMessage> = (0..5)
            .map(|i| ChatMessage {
                role: Role::User,
                content: Some(format!("msg {}", i)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            })
            .collect();
        assert!(!c.needs_compaction(&few, 0.90));
    }
}
