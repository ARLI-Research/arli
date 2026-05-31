use tokio::sync::mpsc;
use std::sync::Arc;
use tracing::{info, debug, warn};

use crate::compaction::Compactor;
use crate::context::PressureLevel;
use crate::error::{Error, Result};
use crate::guardrail::{GuardDecision, Guardrail, ToolCallRecord};
use crate::policy::{Decision, PolicyEngine};
use crate::providers::{
    ChatMessage, LlmResponseContent, Provider, Role, ToolResult,
};
use crate::session::SessionStore;
use crate::tools::ToolRegistry;

/// Messages that the Agent actor can receive
#[derive(Debug)]
pub enum AgentMessage {
    UserMessage(String),
    ToolResult(ToolResult),
    Pause,
    Resume,
    Stop,
    /// Redirect the agent to a new goal (injects into conversation)
    Redirect(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentState {
    Idle,
    Running,
    Paused,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub name: String,
    pub session_id: Option<String>,
    pub system_prompt: Option<String>,
    /// Messages to protect from compaction (most recent N)
    pub protect_last_n: usize,
    /// Messages to protect at start (system prompt, etc.)
    pub protect_first_n: usize,
    /// Token budget (None = unlimited)
    pub token_budget: Option<usize>,
    /// Time budget in seconds (None = unlimited)
    pub time_budget_secs: Option<u64>,
    /// Dollar budget in USD cents (None = unlimited)
    pub dollar_budget_cents: Option<u64>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: "agent".to_string(),
            session_id: None,
            system_prompt: None,
            protect_last_n: 20,
            protect_first_n: 3,
            token_budget: None,
            time_budget_secs: None,
            dollar_budget_cents: None,
        }
    }
}

/// The Agent actor — the core of ARLI.
pub struct Agent {
    pub config: AgentConfig,
    pub state: AgentState,

    provider: Box<dyn Provider>,
    tools: ToolRegistry,
    policy: PolicyEngine,
    session: Option<SessionStore>,
    session_id: Option<String>,

    messages: Vec<ChatMessage>,
    max_iterations: usize,
    iteration: usize,

    /// How many times we've compacted
    compaction_count: usize,

    /// Budget tracking
    tokens_consumed: usize,
    start_time: std::time::Instant,
    /// Grace period: after budget exceeded, one final call allowed
    grace_period_used: bool,

    rx: mpsc::Receiver<AgentMessage>,
    tx: mpsc::Sender<AgentMessage>,

    /// Safety guardrail (Pre-Reply checkpoint from AgentDoG 1.5)
    guardrail: Option<Arc<Guardrail>>,
    /// Tool call history for the current trajectory
    tool_history: Vec<ToolCallRecord>,
}

impl Agent {
    pub fn new(
        config: AgentConfig,
        provider: Box<dyn Provider>,
        tools: ToolRegistry,
        policy: PolicyEngine,
        session: Option<SessionStore>,
        max_iterations: usize,
        guardrail: Option<Arc<Guardrail>>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(64);
        Self {
            config,
            state: AgentState::Idle,
            provider,
            tools,
            policy,
            session,
            session_id: None,
            messages: Vec::new(),
            max_iterations,
            iteration: 0,
            compaction_count: 0,
            tokens_consumed: 0,
            start_time: std::time::Instant::now(),
            grace_period_used: false,
            rx,
            tx,
            guardrail,
            tool_history: Vec::new(),
        }
    }

    pub fn sender(&self) -> mpsc::Sender<AgentMessage> {
        self.tx.clone()
    }

    fn build_system_prompt(name: &str) -> String {
        format!(
            r#"You are {name}, an AI agent built on the ARLI harness.

You have access to tools for reading/writing files and executing shell commands.
Use tools when you need to interact with the system. Be direct and concise.

When reading files, use offset/limit for large files (1-indexed lines).
When writing files, provide the full content.
When executing shell commands, be specific and check exit codes.

If you don't need a tool to answer, respond directly without calling tools."#,
            name = name
        )
    }

    /// Load identity from `~/.arli/soul.md` if it exists.
    /// This is analogous to Hermes's soul.md — the agent's core personality.
    fn load_soul_md() -> Option<String> {
        let home = std::env::var("ARLI_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                std::path::PathBuf::from(home).join(".arli")
            });
        let soul_path = home.join("soul.md");
        if soul_path.exists() {
            match std::fs::read_to_string(&soul_path) {
                Ok(content) => {
                    let trimmed = content.trim().to_string();
                    if !trimmed.is_empty() {
                        tracing::info!("Loaded identity from {}", soul_path.display());
                        return Some(trimmed);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to read {}: {}", soul_path.display(), e);
                }
            }
        }
        None
    }

    /// Assemble the 3-tier system prompt.
    ///
    /// Priority for Tier 1 (identity):
    ///   1. config.system_prompt (explicitly set)
    ///   2. ~/.arli/soul.md (identity file, like Hermes)
    ///   3. Default built-in prompt
    ///
    /// Tier 2 (context): Working directory + project files.
    /// Tier 3 (volatile): Memory, metrics, timestamp.
    fn assemble_system_prompt(&self) -> String {
        let mut parts = Vec::new();

        // Tier 1: Stable identity
        if let Some(ref custom) = self.config.system_prompt {
            // Explicitly configured prompt (highest priority)
            parts.push(custom.clone());
        } else {
            // Check for ~/.arli/soul.md
            let soul_md = Self::load_soul_md();
            if let Some(soul) = soul_md {
                parts.push(soul);
            } else {
                // Fallback to built-in
                parts.push(Self::build_system_prompt(&self.config.name));
            }
        }

        // Tier 2: Context — working directory + project files
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        parts.push(format!("Current working directory: {}", cwd));

        // Inject project context files (AGENTS.md, CLAUDE.md, etc.)
        if let Some(project_ctx) = Self::load_project_context() {
            parts.push(project_ctx);
        }

        // Tier 3: Volatile state
        let token_count = self.provider.count_message_tokens(&self.messages);
        let pressure = self.provider.pressure(&self.messages);
        parts.push(format!(
            "Context: {:.0}% of {}-token window used ({} tokens)",
            pressure * 100.0,
            self.provider.context_window(),
            token_count
        ));

        if self.compaction_count > 0 {
            parts.push(format!(
                "Note: This conversation has been compacted {} time(s).",
                self.compaction_count
            ));
        }

        parts.join("\n\n")
    }

    /// Scan the current working directory for project context files.
    ///
    /// Looks for: AGENTS.md, CLAUDE.md, .cursorrules, .github/copilot-instructions.md
    /// Returns a formatted Tier 2 context block if any are found.
    fn load_project_context() -> Option<String> {
        let cwd = std::env::current_dir().ok()?;

        let candidates = [
            "AGENTS.md",
            "CLAUDE.md",
            ".cursorrules",
            ".github/copilot-instructions.md",
            "README.md",
        ];

        let mut found = Vec::new();

        for name in &candidates {
            let path = cwd.join(name);
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        // Truncate very large files
                        let truncated = if content.len() > 8000 {
                            format!("{}...\n[truncated at 8000 chars, {} total]", 
                                &content[..8000], content.len())
                        } else {
                            content
                        };
                        found.push(format!(
                            "--- {} ---\n{}",
                            name,
                            truncated
                        ));
                    }
                    Err(_) => {}
                }
            }
        }

        if found.is_empty() {
            None
        } else {
            Some(format!(
                "Project context files found in working directory:\n\n{}\n\n\
                 Use these to understand the project structure, conventions, and constraints.",
                found.join("\n\n")
            ))
        }
    }

    pub async fn run(&mut self, initial_message: Option<String>) -> Result<String> {
        info!("Agent '{}' starting", self.config.name);

        // If no session_id yet (fresh start), create a new session.
        // If load_history() was called, the session_id is already set.
        if self.session_id.is_none() {
            if let Some(ref session) = self.session {
                let sid = session.create_session(Some(&self.config.name))?;
                self.session_id = Some(sid.clone());
                info!("Session created: {}", sid);
            }
        }

        // Assemble and inject system prompt
        let system_prompt = self.assemble_system_prompt();
        self.messages.push(ChatMessage::system(system_prompt));

        if let Some(msg) = initial_message {
            self.messages.push(ChatMessage::user(msg.clone()));
            self.save_message(&ChatMessage::user(msg));
            self.state = AgentState::Running;
            return self.process_loop().await;
        }

        self.process_mailbox().await
    }

    async fn process_mailbox(&mut self) -> Result<String> {
        loop {
            tokio::select! {
                msg = self.rx.recv() => {
                    match msg {
                        Some(AgentMessage::UserMessage(text)) => {
                            self.messages.push(ChatMessage::user(text.clone()));
                            self.save_message(&ChatMessage::user(text));
                            self.state = AgentState::Running;
                            return self.process_loop().await;
                        }
                        Some(AgentMessage::Stop) | None => {
                            info!("Agent '{}' stopped", self.config.name);
                            self.state = AgentState::Completed;
                            return Ok("Agent stopped".to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Main processing loop with context pressure awareness.
    async fn process_loop(&mut self) -> Result<String> {
        self.iteration = 0;

        loop {
            if self.iteration >= self.max_iterations {
                warn!("Agent hit max iterations ({})", self.max_iterations);
                self.state = AgentState::Failed;
                return Err(Error::BudgetExceeded {
                    max_iterations: self.max_iterations,
                });
            }

            // --- BUDGET CHECK ---
            let budget_exceeded = self.check_budget();
            if budget_exceeded && self.grace_period_used {
                warn!("Budget exceeded and grace period used — stopping");
                self.state = AgentState::Failed;
                return Err(Error::BudgetExceeded {
                    max_iterations: self.max_iterations,
                });
            }
            if budget_exceeded && !self.grace_period_used {
                info!("Budget exceeded — entering grace period (one final call)");
                self.grace_period_used = true;
                self.messages.push(ChatMessage::user(
                    "[SYSTEM: Budget exceeded (tokens/time/dollars). This is your FINAL response. \
                     Summarize your current state, any pending work, and wrap up concisely.]"
                ));
            }

            self.iteration += 1;

            // Check for steering commands
            if let Ok(msg) = self.rx.try_recv() {
                match msg {
                    AgentMessage::Stop => {
                        self.state = AgentState::Completed;
                        return Ok("Agent stopped by user".to_string());
                    }
                    AgentMessage::Pause => {
                        self.state = AgentState::Paused;
                        loop {
                            match self.rx.recv().await {
                                Some(AgentMessage::Resume) | Some(AgentMessage::UserMessage(_)) => {
                                    self.state = AgentState::Running;
                                    break;
                                }
                                Some(AgentMessage::Stop) | None => {
                                    self.state = AgentState::Completed;
                                    return Ok("Agent stopped while paused".to_string());
                                }
                                _ => {}
                            }
                        }
                    }
                    AgentMessage::Redirect(new_goal) => {
                        info!("Agent redirected to: {}", new_goal);
                        self.messages.push(ChatMessage::user(format!(
                            "[REDIRECT: New objective — {}]",
                            new_goal
                        )));
                    }
                    _ => {}
                }
            }

            // --- CONTEXT PRESSURE CHECK ---
            let pressure = self.provider.pressure(&self.messages);
            let level = PressureLevel::from_pressure(pressure);
            let tokens = self.provider.count_message_tokens(&self.messages);

            match level {
                PressureLevel::Critical => {
                    warn!(
                        "CRITICAL context pressure: {:.0}% ({}/{} tokens) — compacting now",
                        pressure * 100.0,
                        tokens,
                        self.provider.context_window()
                    );

                    // Trigger automatic compaction
                    let compactor = Compactor::new(
                        self.config.protect_last_n,
                        self.config.protect_first_n,
                    );

                    match compactor.compact(self.provider.as_ref(), &self.messages).await {
                        Ok(Some(result)) => {
                            info!(
                                "Compacted {} messages: {} → {} tokens ({:.0}% reduction)",
                                result.compacted_count,
                                result.tokens_before,
                                result.tokens_after,
                                (1.0 - result.tokens_after as f64 / result.tokens_before as f64) * 100.0
                            );
                            
                            // Record compaction in session lineage
                            if let (Some(ref session), Some(ref sid)) = (&self.session, &self.session_id) {
                                if let Err(e) = session.record_compaction(sid, &result.summary) {
                                    warn!("Failed to record compaction: {}", e);
                                }
                            }

                            // Rebuild messages from compaction result
                            self.rebuild_after_compaction(&compactor, &result);
                            self.compaction_count += 1;

                            // Continue the loop with compacted context
                            continue;
                        }
                        Ok(None) => {
                            debug!("Compaction not needed (insufficient messages)");
                            // Fall through — inject warning so the model knows
                            self.messages.push(ChatMessage::user(format!(
                                "[SYSTEM: Context window is {:.0}% full ({} of {} tokens). \
                                 Not enough messages to compact. Finish soon.]",
                                pressure * 100.0,
                                tokens,
                                self.provider.context_window()
                            )));
                        }
                        Err(e) => {
                            warn!("Compaction failed: {} — injecting warning", e);
                            self.messages.push(ChatMessage::user(format!(
                                "[SYSTEM: Context window is {:.0}% full ({} of {} tokens). \
                                 Compaction failed: {}. Finish soon.]",
                                pressure * 100.0,
                                tokens,
                                self.provider.context_window(),
                                e
                            )));
                        }
                    }
                }
                PressureLevel::High => {
                    warn!(
                        "HIGH context pressure: {:.0}% ({}/{} tokens)",
                        pressure * 100.0,
                        tokens,
                        self.provider.context_window()
                    );
                }
                PressureLevel::Elevated => {
                    debug!(
                        "Elevated context pressure: {:.0}% ({}/{} tokens)",
                        pressure * 100.0,
                        tokens,
                        self.provider.context_window()
                    );
                }
                PressureLevel::Normal => {}
            }

            // Get tool schemas
            let schemas = self.tools.schemas();

            debug!(
                "Turn {}/{}: {} messages ({} tokens, {:.0}%), {} tools",
                self.iteration,
                self.max_iterations,
                self.messages.len(),
                tokens,
                pressure * 100.0,
                schemas.len()
            );

            // Call LLM
            let response = self.provider.chat(&self.messages, &schemas).await?;

            if let Some(ref usage) = response.usage {
                self.tokens_consumed += usage.total_tokens as usize;
                debug!(
                    "LLM: {}p + {}c = {}t (total consumed: {})",
                    usage.prompt_tokens, usage.completion_tokens, usage.total_tokens,
                    self.tokens_consumed
                );
            }

            match response.content {
                LlmResponseContent::Text { content } => {
                    info!("Agent completed: {} chars", content.len());

                    // --- PRE-REPLY GUARDRAIL (AgentDoG 1.5) ---
                    let final_re = if let Some(ref guard) = self.guardrail {
                        let decision = guard
                            .evaluate(&self.messages, &content, &self.tool_history)
                            .await;
                        match decision {
                            GuardDecision::Safe => content,
                            GuardDecision::Unsafe { classification, replacement } => {
                                warn!(
                                    "Guardrail BLOCKED reply: {:?} / {:?} / {:?}",
                                    classification.risk_source,
                                    classification.failure_mode,
                                    classification.real_world_harm
                                );
                                replacement
                            }
                        }
                    } else {
                        content
                    };

                    let msg = ChatMessage::assistant_text(&final_re);
                    self.save_message(&msg);
                    self.messages.push(msg);
                    self.state = AgentState::Completed;
                    return Ok(final_re);
                }
                LlmResponseContent::ToolCalls {
                    content: _text,
                    tool_calls,
                } => {
                    let assistant_msg = ChatMessage::assistant_with_tools(tool_calls.clone());
                    self.save_message(&assistant_msg);
                    self.messages.push(assistant_msg);

                    for tc in &tool_calls {
                        info!("Tool: {} ({})", tc.function.name, tc.id);

                        // Parse arguments for policy check
                        let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(serde_json::json!({}));

                        // --- POLICY CHECK ---
                        let decision = self.policy.evaluate(
                            &tc.function.name,
                            &args,
                            Some(&self.config.name),
                        );

                        let result_content = match decision {
                            Decision::Allow => {
                                // --- RATE LIMIT CHECK ---
                                let rate_decision = self.policy.check_rate_limit(&tc.function.name);
                                match rate_decision {
                                    Decision::RateLimited { ref reason, retry_after_secs } => {
                                        warn!("Tool '{}' RATE LIMITED: {}", tc.function.name, reason);
                                        format!(
                                            "RATE LIMITED: {}
Try again in {} seconds.",
                                            reason, retry_after_secs
                                        )
                                    }
                                    _ => {
                                        let output = self
                                            .tools
                                            .execute(&tc.function.name, &tc.function.arguments)
                                            .await;

                                        if output.success {
                                            output.content
                                        } else {
                                            format!(
                                                "Error: {}",
                                                output.error.as_deref().unwrap_or("Unknown error")
                                            )
                                        }
                                    }
                                }
                            }
                            Decision::Deny { ref reason } => {
                                warn!(
                                    "Tool '{}' DENIED by policy: {}",
                                    tc.function.name, reason
                                );
                                format!(
                                    "ACCESS DENIED by policy engine: {}\n\
                                     Tool '{}' is blocked. Use a different approach.",
                                    reason, tc.function.name
                                )
                            }
                            Decision::NeedsApproval { ref reason } => {
                                warn!(
                                    "Tool '{}' NEEDS APPROVAL: {}",
                                    tc.function.name, reason
                                );
                                format!(
                                    "APPROVAL REQUIRED: {}\n\
                                     The tool '{}' needs human confirmation to proceed.\n\
                                     This execution was BLOCKED. Request approval and retry.",
                                    reason, tc.function.name
                                )
                            }
                            Decision::RateLimited { ref reason, .. } => {
                                format!("RATE LIMITED: {}", reason)
                            }
                        };

                        // Record tool call for trajectory audit (AgentDoG 1.5)
                        self.tool_history.push(ToolCallRecord::new(
                            &tc.function.name,
                            Some(tc.function.arguments.clone()),
                            Some(result_content.clone()),
                            !result_content.starts_with("Error:")
                                && !result_content.starts_with("ACCESS DENIED")
                                && !result_content.starts_with("APPROVAL REQUIRED")
                                && !result_content.starts_with("RATE LIMITED"),
                        ));

                        let tool_result = ToolResult::new(tc.id.clone(), result_content);
                        let result_msg = tool_result.as_message();
                        self.save_message(&result_msg);
                        self.messages.push(result_msg);
                    }
                }
            }
        }
    }

    fn save_message(&self, message: &ChatMessage) {
        if let (Some(ref session), Some(ref session_id)) = (&self.session, &self.session_id) {
            if let Err(e) = session.save_message(session_id, message) {
                warn!("Failed to save message: {}", e);
            }
        }
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Load history from a previous session — enables `arli --resume`.
    ///
    /// Strips the old system prompt (first message if it's System role) so the agent
    /// can assemble a fresh one. The parent_session_id is stored for session lineage.
    pub fn load_history(&mut self, session_id: String, mut messages: Vec<ChatMessage>) {
        // Strip the old system prompt — we'll rebuild it fresh in run()
        if messages.first().map(|m| m.role == Role::System).unwrap_or(false) {
            messages.remove(0);
        }
        self.session_id = Some(session_id);
        self.messages = messages;
        info!(
            "Loaded {} messages from previous session {}",
            self.messages.len(),
            self.session_id.as_deref().unwrap_or("?")
        );
    }

    /// Check if any budget has been exceeded.
    fn check_budget(&self) -> bool {
        if let Some(token_budget) = self.config.token_budget {
            if self.tokens_consumed >= token_budget {
                return true;
            }
        }
        if let Some(time_budget) = self.config.time_budget_secs {
            if self.start_time.elapsed().as_secs() >= time_budget {
                return true;
            }
        }
        false
    }

    /// Rebuild the message list after compaction.
    fn rebuild_after_compaction(
        &mut self,
        _compactor: &Compactor,
        result: &crate::context::CompactionResult,
    ) {
        let old_len = self.messages.len();
        self.messages = result.messages.clone();
        debug!(
            "Rebuilt messages after compaction: {} → {}",
            old_len,
            self.messages.len()
        );
    }
}
