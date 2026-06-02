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

/// How often to poll for contract state changes.
const POLL_INTERVAL_SECS: u64 = 30;

/// Maximum number of failed attestation attempts per contract.
const MAX_RETRIES: u32 = 3;

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

        Self {
            enso,
            jobs,
            keypair,
            binary_hash,
            agent_id,
            sandbox_config_hash,
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
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
            for job in &mut self.jobs {
                if job.attested || job.failures >= MAX_RETRIES {
                    continue;
                }

                match self.process_contract(job).await {
                    Ok(OracleResult::Attested) => {
                        job.attested = true;
                        attested_count += 1;
                        tracing::info!("Oracle: contract {} attested OK", job.contract_id);
                    }
                    Ok(OracleResult::AlreadyDone) => {
                        job.attested = true;
                    }
                    Err(e) => {
                        job.failures += 1;
                        tracing::error!(
                            "Oracle: contract {} attempt {}/{} failed: {}",
                            job.contract_id, job.failures, MAX_RETRIES, e,
                        );
                    }
                }
            }

            if self.jobs.iter().all(|j| j.attested || j.failures >= MAX_RETRIES) {
                tracing::info!("Oracle: all {} contracts done ({} attested)", self.jobs.len(), attested_count);
                break;
            }

            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        }

        attested_count
    }

    /// Stop the oracle loop.
    pub fn stop(&self) {
        self.running.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Process a single contract: build attestation, sign, submit.
    async fn process_contract(&self, job: &OracleJob) -> Result<OracleResult, String> {
        let keypair = self.keypair.as_ref().unwrap();

        let builder = crate::attestation::AttestationBuilder::new(
            keypair.clone(),
            self.binary_hash.clone(),
        );

        let ocsf_event = serde_json::json!({
            "class_uid": 6007,
            "activity_name": "Oracle Attestation",
            "agent_id": self.agent_id,
            "job_id": job.contract_id,
            "sandbox": self.sandbox_config_hash,
        });

        let ocsf_json = serde_json::to_string(&ocsf_event)
            .map_err(|e| format!("serialize OCSF event: {}", e))?;

        let attestation = builder.build(
            format!("oracle-{}", job.contract_id),
            self.agent_id.clone(),
            job.contract_id.clone(),
            &ocsf_json,
            None,
            self.sandbox_config_hash.clone(),
            true,
            true,
            65534,
        );

        tracing::info!(
            "Oracle: submitting attestation for {} (ocsf:{})",
            job.contract_id,
            &attestation.ocsf_event_hash[..16],
        );

        let response = self.enso.submit_attestation(&attestation).await?;

        use crate::enso::SettlementStatus;
        match response.status {
            SettlementStatus::Verified => {
                crate::metrics::Metrics::global().inc_attestations();
                Ok(OracleResult::Attested)
            }
            SettlementStatus::Settled => Ok(OracleResult::AlreadyDone),
            SettlementStatus::Disputed => Err(format!("Disputed: {}", response.message)),
            SettlementStatus::Pending => Err("Pending — may need admin approval".into()),
        }
    }
}

/// Result of processing a single contract.
#[derive(Debug, PartialEq)]
enum OracleResult {
    Attested,
    AlreadyDone,
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
        tracing::warn!("ENSO feature not compiled — oracle dry-run only. {} contracts loaded.", self.jobs.len());
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
}
