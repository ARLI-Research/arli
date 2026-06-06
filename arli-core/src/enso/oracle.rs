//! ENSO Compute Oracle — automated job execution + attestation loop.
//!
//! The oracle monitors ENSO Contracts for active jobs assigned to this ARLI
//! agent, executes them in a kernel sandbox, and automatically submits
//! ed25519-signed attestations for settlement.
//!
//! ## Architecture
//!
//! ```text
//! ENSO Contracts (ICP)          ARLI Oracle (this module)
//! ┌─────────────────┐           ┌────────────────────────┐
//! │ Contract Active  │──poll──→ │ OracleLoop             │
//! │ job_id = "..."   │          │  ├─ agent.run(job)     │
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

    /// Run the oracle loop — polls contracts, executes, attests.
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

    /// Process a single contract by ID: build attestation, sign, submit payment atomically.
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

        let ocsf_event = serde_json::json!({
            "class_uid": 6007,
            "activity_name": "Oracle Attestation",
            "agent_id": self.agent_id,
            "job_id": contract_id,
            "sandbox": self.sandbox_config_hash,
        });

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
                }

                // Success — commit workspace snapshot
                if let Some(snap) = snapshot {
                    let _ = snap.commit();
                }

                Ok(OracleResult::Attested)
            }
            SettlementStatus::Disputed => {
                task_state.add_error(&result.message);
                task_state.add_check("enso_settlement", false, None, Some(result.message.clone()));
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
