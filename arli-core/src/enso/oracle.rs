//! ENSO Compute Oracle — automated job execution + attestation loop.
//!
//! The oracle monitors ENSO Contracts for active jobs assigned to this ARLI
//! agent, executes them via a pluggable [ExecutionHandler], and automatically
//! submits ed25519-signed attestations for settlement.
//!
//! ## Architecture
//!
//! ```text
//! ENSO Contracts (ICP)          ARLI Oracle (this module)
//! ┌─────────────────┐           ┌────────────────────────┐
//! │ Contract Active  │──poll──→ │ OracleLoop             │
//! │ job_id = "..."   │          │  ├─ handler.execute()  │ ← pluggable
//! │ sandbox reqs     │          │  ├─ build_attestation  │
//! └─────────────────┘          │  └─ submit → Verified  │
//!                               └────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```bash
//! ENSO_CONTRACTS=contract_xxx,contract_yyy arli enso oracle
//! ```

use std::time::Duration;

use async_trait::async_trait;

/// Base poll interval in seconds.
const POLL_INTERVAL_SECS: u64 = 30;

/// Maximum number of failed attestation attempts per contract.
const MAX_RETRIES: u32 = 3;

/// Calculate exponential backoff delay for retry N (0-indexed).
/// Base: 30s, doubles each retry, capped at 5 minutes.
pub fn backoff_delay(retry: u32) -> Duration {
    let base = 30u64;
    let delay = base * 2u64.pow(retry.min(4)); // 30, 60, 120, 240, 480
    Duration::from_secs(delay.min(300)) // capped at 5 min
}

// ============================================================================
// ALWAYS-AVAILABLE TYPES
// ============================================================================

/// A single oracle job — one ENSO contract to monitor and attest.
#[derive(Debug, Clone)]
pub struct OracleJob {
    /// ENSO contract job_id
    pub contract_id: String,
    /// Whether this contract has been attested already
    pub attested: bool,
    /// Number of failed attestation attempts
    pub failures: u32,
}

impl OracleJob {
    pub fn new(contract_id: String) -> Self {
        Self {
            contract_id,
            attested: false,
            failures: 0,
        }
    }
}

/// Load oracle contracts from env var `ENSO_CONTRACTS`.
///
/// Format: comma-separated contract IDs.
/// Example: `ENSO_CONTRACTS=contract_xxx,contract_yyy`
pub fn load_contracts_from_env() -> Vec<String> {
    std::env::var("ENSO_CONTRACTS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Result of executing a contract — returned by [ExecutionHandler].
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// OCSF event JSON (the attestation payload).
    /// Must contain at minimum: class_uid, activity_name, agent_id, job_id.
    pub ocsf_event: serde_json::Value,
    /// Optional artifacts produced by execution (paths to output files).
    pub artifacts: Vec<String>,
    /// Whether execution was successful.
    pub success: bool,
    /// Optional execution metrics for ENSO SLA enforcement.
    ///
    /// Populated by the execution handler with measured values
    /// (e.g., execution_latency_ms, trades_evaluated, pnl_usd).
    /// ENSO's `check_sla` compares these against contract thresholds.
    pub metrics: Option<serde_json::Value>,
}

/// Pluggable contract execution handler.
///
/// The ENSO oracle calls `execute()` for each active contract
/// to produce a real OCSF attestation event instead of a dummy one.
///
/// # Implementations
///
/// - **Trading agent**: runs a trading strategy, returns trade log as OCSF event.
/// - **Code agent**: compiles/runs code in sandbox, returns test results.
/// - **ML agent**: trains/evaluates a model, returns metrics.
///
/// # Async
///
/// Handlers are async — trading agents need network I/O for market data
/// and order execution. The oracle awaits each handler's completion.
///
/// # Thread safety
///
/// Handlers must be `Send + Sync` — the oracle runs in an async context
/// and may process multiple contracts concurrently.
#[async_trait::async_trait]
pub trait ExecutionHandler: Send + Sync {
    /// Execute the contract and return an OCSF attestation event.
    ///
    /// `job` contains the full job specification from ENSO (job_type, job_params, SLA, etc.).
    async fn execute(
        &self,
        contract_id: &str,
        agent_id: &str,
        job: &crate::enso::JobDetail,
    ) -> Result<ExecutionResult, String>;
}

/// Helper: build a minimal OCSF event when no execution handler is configured.
pub fn dummy_ocsf_event(contract_id: &str, agent_id: &str, sandbox_hash: &str) -> serde_json::Value {
    serde_json::json!({
        "class_uid": 6007,
        "activity_name": "Oracle Attestation",
        "agent_id": agent_id,
        "job_id": contract_id,
        "sandbox": sandbox_hash,
    })
}

// ============================================================================
// ENSO ORACLE (requires `enso` feature for full functionality)
// ============================================================================

/// ENSO compute oracle — polls for active contracts, executes, attests.
#[cfg(feature = "enso")]
pub struct EnsoOracle {
    /// ENSO client
    enso: crate::enso::EnsoClient,
    /// Jobs to monitor
    jobs: Vec<OracleJob>,
    /// ARLI signing keypair
    keypair: Option<crate::attestation::ArliKeypair>,
    /// ARLI binary hash for attestation
    binary_hash: String,
    /// Agent ID registered with ENSO
    agent_id: String,
    /// Sandbox config hash
    sandbox_config_hash: String,
    /// Whether to stop polling
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// SQLite connection for run_id ↔ contract_id persistence
    db: Option<rusqlite::Connection>,
    /// Shared harness state — per-contract execution trail for ENSO visibility (§4.3)
    task_states: std::collections::HashMap<String, crate::task_state::TaskState>,
    /// Workspace root for fault-tolerant snapshots.
    workspace_root: Option<std::path::PathBuf>,
    /// Verification pipeline (compile→lint→test→fuzz) to run before attestation.
    /// When set, the pipeline must pass all required steps for attestation to proceed.
    verification_pipeline: Option<crate::verification_pipeline::VerificationPipeline>,
    /// Pluggable execution handler — produces real OCSF events instead of dummy ones.
    /// When [None], the oracle uses [dummy_ocsf_event] for attestation.
    execution_handler: Option<Box<dyn ExecutionHandler>>,
    /// Optional notification sink for external alerts (Telegram, webhook, etc.).
    /// Fires on settled, disputed, SLA penalty events.
    notification_sink: Option<Box<dyn NotificationSink>>,
}

#[cfg(feature = "enso")]
impl EnsoOracle {
    /// Create a new oracle instance.
    pub fn new(
        contract_ids: Vec<String>,
        agent_id: String,
        binary_hash: String,
        sandbox_config_hash: String,
        keypair: Option<crate::attestation::ArliKeypair>,
        enso: crate::enso::EnsoClient,
    ) -> Self {
        let jobs: Vec<OracleJob> = contract_ids.into_iter().map(OracleJob::new).collect();

        tracing::info!(
            "Oracle initialized: {} contracts, agent={}",
            jobs.len(),
            agent_id,
        );

        // Initialize SQLite for run_id ↔ contract_id persistence
        let db = Self::init_db();

        Self {
            enso,
            jobs,
            keypair,
            binary_hash,
            agent_id,
            sandbox_config_hash,
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            db,
            task_states: std::collections::HashMap::new(),
            workspace_root: None,
            verification_pipeline: None,
            execution_handler: None,
            notification_sink: None,
        }
    }

    /// Initialize SQLite table for attestation traceability.
    fn init_db() -> Option<rusqlite::Connection> {
        let db_path = crate::config::arli_home().join("enso_oracle.db");
        match rusqlite::Connection::open(&db_path) {
            Ok(conn) => {
                let _ = conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS attestations (
                        run_id TEXT PRIMARY KEY,
                        contract_id TEXT NOT NULL,
                        agent_id TEXT NOT NULL,
                        ocsf_event_hash TEXT NOT NULL,
                        attested_at TEXT NOT NULL,
                        tx_id TEXT,
                        status TEXT NOT NULL
                    );
                    CREATE INDEX IF NOT EXISTS idx_att_contract ON attestations(contract_id);",
                );
                tracing::info!("Oracle DB initialized at {}", db_path.display());
                Some(conn)
            }
            Err(e) => {
                tracing::warn!(
                    "Oracle DB unavailable ({}), run_id mapping not persisted",
                    e
                );
                None
            }
        }
    }

    /// Persist run_id ↔ contract_id mapping after successful attestation.
    fn save_attestation_mapping(
        &self,
        run_id: &str,
        contract_id: &str,
        ocsf_hash: &str,
        tx_id: Option<&str>,
    ) {
        if let Some(ref db) = self.db {
            let status = if tx_id.is_some() {
                "settled"
            } else {
                "verified"
            };
            let _ = db.execute(
                "INSERT OR REPLACE INTO attestations (run_id, contract_id, agent_id, ocsf_event_hash, attested_at, tx_id, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    run_id,
                    contract_id,
                    self.agent_id,
                    ocsf_hash,
                    chrono::Utc::now().to_rfc3339(),
                    tx_id.unwrap_or(""),
                    status,
                ],
            );
        }
    }

    /// Run the oracle loop — auto-discovers contracts from ENSO, executes, attests.
    ///
    /// On each poll cycle, calls `list_active_contracts_for_agent` on the ENSO canister
    /// to discover new contracts. Already-known contracts from `ENSO_CONTRACTS` env var
    /// are also monitored.
    ///
    /// Blocks until `stop()` is called or all jobs complete.
    /// Returns the number of contracts that were successfully attested.
    pub async fn run(&mut self) -> usize {
        if self.keypair.is_none() {
            tracing::warn!("No ARLI keypair loaded — oracle cannot sign attestations");
            return 0;
        }

        let mut attested_count = 0;

        while self.running.load(std::sync::atomic::Ordering::Relaxed) {
            // --- Auto-discover new contracts from ENSO canister ---
            match self
                .enso
                .list_active_contracts_for_agent(&self.agent_id)
                .await
            {
                Ok(active_ids) => {
                    for cid in &active_ids {
                        if !self.jobs.iter().any(|j| &j.contract_id == cid) {
                            tracing::info!("Oracle: discovered new contract {}", cid);
                            self.jobs.push(OracleJob::new(cid.clone()));
                        }
                    }
                    tracing::debug!(
                        "Oracle: {} total jobs ({} active on ENSO)",
                        self.jobs.len(),
                        active_ids.len(),
                    );
                }
                Err(e) => {
                    tracing::warn!("Oracle: failed to list active contracts: {e}");
                }
            }

            // Collect pending contract IDs to avoid borrow conflict with &mut self
            let pending_contracts: Vec<String> = self
                .jobs
                .iter()
                .filter(|j| !j.attested && j.failures < MAX_RETRIES)
                .map(|j| j.contract_id.clone())
                .collect();

            for contract_id in pending_contracts {
                let result = self.process_contract_by_id(&contract_id).await;

                // Find the job index to update
                let idx = self.jobs.iter().position(|j| j.contract_id == contract_id);
                if let Some(idx) = idx {
                    match result {
                        Ok(OracleResult::Attested) => {
                            self.jobs[idx].attested = true;
                            attested_count += 1;
                            tracing::info!(
                                "Oracle: contract {} attested OK",
                                self.jobs[idx].contract_id
                            );
                        }
                        Err(e) => {
                            self.jobs[idx].failures += 1;
                            let delay = backoff_delay(self.jobs[idx].failures);
                            tracing::error!(
                                "Oracle: contract {} attempt {}/{} failed: {}. Backoff: {:?}",
                                self.jobs[idx].contract_id,
                                self.jobs[idx].failures,
                                MAX_RETRIES,
                                e,
                                delay,
                            );
                            tokio::time::sleep(delay).await;
                        }
                    } // close match
                } // close if let
            } // close for contract_id

            if self
                .jobs
                .iter()
                .all(|j| j.attested || j.failures >= MAX_RETRIES)
            {
                tracing::info!(
                    "Oracle: all {} contracts done ({} attested)",
                    self.jobs.len(),
                    attested_count
                );
                break;
            }

            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        }

        attested_count
    }

    /// Stop the oracle loop.
    pub fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Set workspace root for fault-tolerant snapshots.
    /// When set, the oracle creates a snapshot before each contract execution
    /// and rolls back on failure (Fault-Tolerant Sandboxing, 2025).
    pub fn set_workspace_root(&mut self, path: std::path::PathBuf) {
        self.workspace_root = Some(path);
    }

    /// Set a verification pipeline to run before attestation.
    ///
    /// When set, `process_contract_by_id()` runs compile→lint→test→fuzz
    /// and blocks attestation if any required step fails.
    /// Pass `None` to disable verification.
    pub fn set_verification_pipeline(
        &mut self,
        pipeline: Option<crate::verification_pipeline::VerificationPipeline>,
    ) {
        self.verification_pipeline = pipeline;
    }

    /// Set a pluggable execution handler to produce real OCSF attestation events.
    ///
    /// When set, the oracle calls `handler.execute(contract_id, agent_id)`
    /// instead of using [dummy_ocsf_event]. Use this to wire in:
    ///
    /// - **Trading agents**: execute strategy → trade log as OCSF
    /// - **Code agents**: compile+test in sandbox → results as OCSF
    /// - **ML agents**: train+evaluate → metrics as OCSF
    ///
    /// Pass `None` to revert to dummy events.
    pub fn set_execution_handler(&mut self, handler: Option<Box<dyn ExecutionHandler>>) {
        self.execution_handler = handler;
    }

    /// Set a notification sink for external alerts (Telegram, webhook, etc.).
    ///
    /// When set, the oracle calls `on_settled`, `on_disputed`, and
    /// `on_sla_penalty` on the sink when those events occur.
    /// Pass `None` to disable notifications.
    pub fn set_notification_sink(&mut self, sink: Option<Box<dyn NotificationSink>>) {
        self.notification_sink = sink;
    }

    /// Process a single contract by ID: execute (via handler), build attestation, sign, submit.
    async fn process_contract_by_id(&mut self, contract_id: &str) -> Result<OracleResult, String> {
        // --- Shared Harness State (§4.3): load or create task state ---
        let mut task_state = self.task_states.remove(contract_id).unwrap_or_else(|| {
            crate::task_state::TaskState::new(
                contract_id,
                &self.agent_id,
                &format!("ENSO contract {}", contract_id),
            )
        });

        task_state.increment_attempts();
        task_state.transition_to(crate::task_state::TaskPhase::Executing);

        // --- Fault-Tolerant Sandbox: snapshot workspace before execution ---
        let snapshot = if let Some(ref ws_root) = self.workspace_root {
            match crate::workspace_snapshot::WorkspaceSnapshot::create(ws_root) {
                Ok(snap) => {
                    tracing::debug!("Workspace snapshot created for {}", contract_id);
                    Some(snap)
                }
                Err(e) => {
                    tracing::warn!(
                        "Workspace snapshot failed ({}), continuing without rollback safety",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        // --- Verification Pipeline: compile→lint→test→fuzz before attestation ---
        if let Some(ref pipeline) = self.verification_pipeline {
            if let Some(ref ws_root) = self.workspace_root {
                tracing::info!(
                    "Running verification pipeline for {} in {}",
                    contract_id,
                    ws_root.display()
                );
                let vr_result = pipeline.run(ws_root);

                // Update task state with verification results
                task_state.transition_to(crate::task_state::TaskPhase::Verifying);
                for step_result in &vr_result.steps {
                    task_state.add_check(
                        &format!("verify_{}", step_result.step.name()),
                        step_result.passed,
                        Some(step_result.exit_code),
                        Some(format!(
                            "duration={}ms, passed={}/{}",
                            step_result.duration_ms,
                            step_result.commands_passed,
                            step_result.commands_count
                        )),
                    );
                }
                let vr_json = serde_json::to_string(&vr_result).unwrap_or_default();
                task_state.add_artifact(
                    "verification_pipeline_result.json",
                    vr_json.len() as u64,
                    None,
                );

                if !vr_result.all_required_passed {
                    let failed = vr_result.failed_steps.join(", ");
                    let msg = format!(
                        "Verification pipeline FAILED: {}. Summary: {}",
                        failed, vr_result.summary
                    );
                    tracing::warn!("{}", msg);
                    task_state.add_error(&msg);

                    // --- Failure Attribution: verification failure ---
                    let attribution = crate::failure_attribution::classify(&msg);
                    task_state.add_error(&format!(
                        "Failure attribution: {} (confidence={:.0}%, pattern='{}')",
                        attribution.category.name(),
                        attribution.confidence * 100.0,
                        attribution.matched_pattern,
                    ));
                    tracing::warn!(
                        "Failure attribution for {}: {:?} (retryable={})",
                        contract_id,
                        attribution.category,
                        attribution.category.is_retryable(),
                    );

                    task_state.transition_to(crate::task_state::TaskPhase::Failed);
                    let _ = task_state.save();
                    self.task_states.insert(contract_id.to_string(), task_state);

                    // Rollback workspace on verification failure
                    if let Some(snap) = snapshot {
                        let _ = snap.rollback();
                    }

                    return Err(msg);
                }

                tracing::info!(
                    "Verification pipeline PASSED for {}: {}",
                    contract_id,
                    vr_result.summary
                );
            }
        }

        let keypair = self.keypair.as_ref().unwrap();

        let builder =
            crate::attestation::AttestationBuilder::new(keypair.clone(), self.binary_hash.clone());

        // --- Execute contract via pluggable handler (or fall back to dummy) ---
        let (ocsf_event, artifacts, handler_metrics) = if let Some(ref handler) = self.execution_handler {
            // Fetch job details from ENSO canister
            let job = self.enso.get_job_details(contract_id).await?;
            tracing::info!(
                "Oracle: executing {} — type={}, params_len={}",
                contract_id,
                job.job_type,
                job.job_params.len(),
            );
            match handler.execute(contract_id, &self.agent_id, &job).await {
                Ok(result) => {
                    tracing::info!(
                        "Oracle: handler executed {} — success={}, artifacts={}",
                        contract_id,
                        result.success,
                        result.artifacts.len(),
                    );
                    for artifact in &result.artifacts {
                        task_state.add_artifact(
                            artifact,
                            0, // size unknown without stat
                            None,
                        );
                    }
                    if !result.success {
                        task_state.add_error("Handler reported execution failure");
                    }
                    (result.ocsf_event, result.artifacts, result.metrics)
                }
                Err(e) => {
                    tracing::error!("Oracle: handler failed for {}: {}", contract_id, e);
                    task_state.add_error(&format!("Handler error: {}", e));
                    return Err(format!("Execution handler failed: {}", e));
                }
            }
        } else {
            (
                dummy_ocsf_event(contract_id, &self.agent_id, &self.sandbox_config_hash),
                Vec::new(),
                None,
            )
        };

        let ocsf_json = serde_json::to_string(&ocsf_event)
            .map_err(|e| format!("serialize OCSF event: {}", e))?;

        let attestation = builder.build(
            format!("oracle-{}", contract_id),
            self.agent_id.clone(),
            contract_id.to_string(),
            &ocsf_json,
            None,
            self.sandbox_config_hash.clone(),
            true,
            true,
            65534,
            None, // task_contract_hash — oracle can't know contract upfront
            handler_metrics,
        );

        let attestation_json = serde_json::to_string(&attestation)
            .map_err(|e| format!("serialize attestation: {}", e))?;

        tracing::debug!(
            "Oracle attestation JSON (first 500 chars): {}",
            &attestation_json[..attestation_json.len().min(500)]
        );
        tracing::info!(
            "Oracle: submitting payment+attestation for {} (ocsf:{}, agent_id={})",
            contract_id,
            &attestation.ocsf_event_hash[..16],
            self.agent_id,
        );

        // One ICP call: verify attestation → settle → release payment
        let result = self
            .enso
            .submit_arli_payment(&contract_id, &attestation_json)
            .await?;

        use crate::enso::SettlementStatus;
        match result.status {
            SettlementStatus::Verified | SettlementStatus::Settled => {
                crate::metrics::Metrics::global().inc_attestations();

                // --- Update shared harness state ---
                let run_id_val = format!("oracle-{}", contract_id);
                task_state.mark_attested(&run_id_val);
                if result.tx_id.is_some() {
                    task_state.mark_settled(result.tx_id.as_ref().unwrap());
                }
                task_state.add_check("enso_settlement", true, None, Some(result.message.clone()));
                if let Err(e) = task_state.save() {
                    tracing::warn!("Failed to save task state for {}: {}", contract_id, e);
                }
                self.task_states.insert(contract_id.to_string(), task_state);

                // Persist run_id ↔ contract_id for traceability
                let run_id = format!("oracle-{}", contract_id);
                self.save_attestation_mapping(
                    &run_id,
                    &contract_id,
                    &attestation.ocsf_event_hash,
                    result.tx_id.as_deref(),
                );

                if let Some(ref tx_id) = result.tx_id {
                    tracing::info!(
                        "Oracle: contract {} settled, payment released. tx={}, amount={}¢",
                        contract_id,
                        tx_id,
                        result.amount_cents,
                    );
                    // Fire notification
                    if let Some(ref sink) = self.notification_sink {
                        sink.on_settled(
                            contract_id,
                            &self.agent_id,
                            Some(tx_id),
                            result.amount_cents,
                        )
                        .await;
                    }
                } else {
                    // Attested but not settled (no tx_id)
                    if let Some(ref sink) = self.notification_sink {
                        sink.on_settled(contract_id, &self.agent_id, None, 0).await;
                    }
                }

                // Success — commit workspace snapshot
                if let Some(snap) = snapshot {
                    let _ = snap.commit();
                }

                Ok(OracleResult::Attested)
            }
            SettlementStatus::Disputed => {
                // Fire notification
                if let Some(ref sink) = self.notification_sink {
                    sink.on_disputed(contract_id, &self.agent_id, &result.message)
                        .await;
                }

                task_state.add_error(&result.message);
                task_state.add_check("enso_settlement", false, None, Some(result.message.clone()));

                // --- Failure Attribution: classify why ENSO rejected ---
                let attribution = crate::failure_attribution::classify(&result.message);
                task_state.add_error(&format!(
                    "Failure attribution: {} (confidence={:.0}%, pattern='{}')",
                    attribution.category.name(),
                    attribution.confidence * 100.0,
                    attribution.matched_pattern,
                ));
                tracing::warn!(
                    "Failure attribution for {}: {:?} (retryable={})",
                    contract_id,
                    attribution.category,
                    attribution.category.is_retryable(),
                );

                let _ = task_state.save();
                self.task_states.insert(contract_id.to_string(), task_state);

                // Failure — rollback workspace
                if let Some(snap) = snapshot {
                    if let Err(e) = snap.rollback() {
                        tracing::error!("Workspace rollback failed for {}: {}", contract_id, e);
                    } else {
                        tracing::info!("Workspace rolled back for {}", contract_id);
                    }
                }

                Err(format!("Disputed: {}", result.message))
            }
            SettlementStatus::Pending => {
                task_state.add_error(&format!("Pending: {}", result.message));
                let _ = task_state.save();
                self.task_states.insert(contract_id.to_string(), task_state);

                // Pending — rollback workspace for clean retry
                if let Some(snap) = snapshot {
                    if let Err(e) = snap.rollback() {
                        tracing::error!("Workspace rollback failed for {}: {}", contract_id, e);
                    }
                }

                Err("Pending — may need admin approval".into())
            }
        }
    }
}

/// Result of processing a single contract.
#[derive(Debug, PartialEq)]
enum OracleResult {
    Attested,
}

// ============================================================================
// NOTIFICATION SINK — optional callback for external alerts (Telegram, etc.)
// ============================================================================

/// Sink for oracle notifications — fires on important events.
///
/// Implementations can send Telegram messages, POST webhooks,
/// write to log files, etc.
#[async_trait]
pub trait NotificationSink: Send + Sync {
    /// Called when a contract is successfully attested + settled.
    async fn on_settled(
        &self,
        contract_id: &str,
        agent_id: &str,
        tx_id: Option<&str>,
        amount_cents: u64,
    );

    /// Called when a contract is disputed (SLA fail, signature reject, etc.).
    async fn on_disputed(&self, contract_id: &str, agent_id: &str, reason: &str);

    /// Called on SLA enforcement action (penalty applied).
    async fn on_sla_penalty(
        &self,
        contract_id: &str,
        agent_id: &str,
        amount_cents: u64,
        reason: &str,
    );
}

/// Telegram notifier — sends alerts to a Telegram chat via Bot API.
///
/// Requires `TELEGRAM_BOT_TOKEN` and `TELEGRAM_ALERT_CHAT_ID` env vars.
pub struct TelegramNotifier {
    bot_token: String,
    chat_id: String,
    client: reqwest::Client,
}

impl TelegramNotifier {
    /// Create from environment variables.
    /// Returns None if token or chat_id are not set.
    pub fn from_env() -> Option<Self> {
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN").ok()?;
        let chat_id = std::env::var("TELEGRAM_ALERT_CHAT_ID").ok()?;
        Some(Self {
            bot_token,
            chat_id,
            client: reqwest::Client::new(),
        })
    }

    async fn send(&self, text: &str) {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token
        );
        let result = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": self.chat_id,
                "text": text,
                "parse_mode": "Markdown",
            }))
            .send()
            .await;

        match result {
            Ok(r) if r.status().is_success() => {
                tracing::debug!("Telegram alert sent: {}", text.chars().take(80).collect::<String>());
            }
            Ok(r) => {
                tracing::warn!(
                    "Telegram alert failed: HTTP {} — {}",
                    r.status(),
                    r.text().await.unwrap_or_default(),
                );
            }
            Err(e) => {
                tracing::warn!("Telegram alert send error: {e}");
            }
        }
    }
}

#[async_trait]
impl NotificationSink for TelegramNotifier {
    async fn on_settled(
        &self,
        contract_id: &str,
        agent_id: &str,
        tx_id: Option<&str>,
        amount_cents: u64,
    ) {
        let amount_usd = amount_cents as f64 / 100.0;
        let msg = if let Some(tx) = tx_id {
            format!(
                "✅ *Contract Settled*\n\
                 Contract: `{}`\n\
                 Agent: `{}`\n\
                 Amount: ${:.2}\n\
                 TX: `{}`",
                contract_id, agent_id, amount_usd, tx,
            )
        } else {
            format!(
                "✅ *Contract Attested*\n\
                 Contract: `{}`\n\
                 Agent: `{}`",
                contract_id, agent_id,
            )
        };
        self.send(&msg).await;
    }

    async fn on_disputed(&self, contract_id: &str, agent_id: &str, reason: &str) {
        let msg = format!(
            "❌ *Contract Disputed*\n\
             Contract: `{}`\n\
             Agent: `{}`\n\
             Reason: {}",
            contract_id, agent_id, reason,
        );
        self.send(&msg).await;
    }

    async fn on_sla_penalty(
        &self,
        contract_id: &str,
        agent_id: &str,
        amount_cents: u64,
        reason: &str,
    ) {
        let amount_usd = amount_cents as f64 / 100.0;
        let msg = format!(
            "⚠️ *SLA Penalty Applied*\n\
             Contract: `{}`\n\
             Agent: `{}`\n\
             Penalty: ${:.2}\n\
             Reason: {}",
            contract_id, agent_id, amount_usd, reason,
        );
        self.send(&msg).await;
    }
}

/// Dry-run oracle stub when ENSO feature not compiled.
#[cfg(not(feature = "enso"))]
pub struct EnsoOracle {
    jobs: Vec<OracleJob>,
}

#[cfg(not(feature = "enso"))]
impl EnsoOracle {
    pub fn new(
        contract_ids: Vec<String>,
        _agent_id: String,
        _binary_hash: String,
        _sandbox_config_hash: String,
        _keypair: Option<crate::attestation::ArliKeypair>,
        _enso: (),
    ) -> Self {
        let jobs = contract_ids.into_iter().map(OracleJob::new).collect();
        Self { jobs }
    }

    pub async fn run(&mut self) -> usize {
        tracing::warn!(
            "ENSO feature not compiled — oracle dry-run only. {} contracts loaded.",
            self.jobs.len()
        );
        0
    }

    pub fn stop(&self) {}
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_contracts_from_env() {
        std::env::set_var("ENSO_CONTRACTS", "c1, c2 ,c3");
        let c = load_contracts_from_env();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0], "c1");
        std::env::remove_var("ENSO_CONTRACTS");
    }

    #[test]
    fn test_load_contracts_empty() {
        std::env::remove_var("ENSO_CONTRACTS");
        assert!(load_contracts_from_env().is_empty());
    }

    #[test]
    fn test_oracle_job() {
        let j = OracleJob::new("contract_1".into());
        assert_eq!(j.contract_id, "contract_1");
        assert!(!j.attested);
        assert_eq!(j.failures, 0);
    }

    #[test]
    fn test_backoff_delay() {
        assert_eq!(backoff_delay(0).as_secs(), 30);
        assert_eq!(backoff_delay(1).as_secs(), 60);
        assert_eq!(backoff_delay(2).as_secs(), 120);
        assert_eq!(backoff_delay(3).as_secs(), 240);
        assert_eq!(backoff_delay(4).as_secs(), 300); // capped at 5 min
        assert_eq!(backoff_delay(10).as_secs(), 300); // stays capped
    }
}
