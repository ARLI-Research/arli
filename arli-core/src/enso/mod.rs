//! ENSO integration — ICP canister client for Agent Registry + Contracts + Oracle.
//!
//! Provides clients for:
//! - ENSO Agent Registry: agent registration, key management
//! - ENSO Contracts: attestation submission for settlement
//! - ENSO Oracle: automated job execution + attestation loop

pub mod marketplace;
pub mod oracle;

use crate::attestation::ArliAttestation;
use serde::{Deserialize, Serialize};

// ============================================================================
// TYPES (mirror ENSO Registry Candid types)
// ============================================================================

/// Trust model variants — must match ENSO Registry enum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TrustModel {
    SelfAttested,
    TEEAttested,
    ZKMLVerified,
    HumanAudited,
    MultiSigGoverned,
    KernelSandbox, // ARLI kernel-level sandbox with OCSF attestation
}

/// Agent capability declaration (matches ENSO Registry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsoCapability {
    pub name: String,
    pub description: String,
    pub input_schema: String,
    pub output_schema: String,
    pub latency_sla_ms: u64,
    pub cost_per_call: u64,
    pub jurisdiction: String,
    pub regulated_data: bool,
}

/// Agent endpoints (matches ENSO Registry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsoEndpoints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a2a_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_endpoint: Option<String>,
}

/// Agent registration payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsoAgentRegistration {
    pub name: String,
    pub description: String,
    pub version: String,
    pub base_model: String,
    pub system_prompt_hash: String,
    pub tool_permissions: Vec<String>,
    pub capabilities: Vec<EnsoCapability>,
    pub trust_model: TrustModel,
    pub endpoints: EnsoEndpoints,
    pub wallet_addresses: Vec<EnsoWalletAddress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsoWalletAddress {
    pub chain: String,
    pub address: String,
}

/// Result of agent registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsoAgentId {
    pub agent_id: String,
}

/// SlaMetric from ENSO Contracts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlaMetric {
    pub name: String,
    pub target: String,
    pub verifier_canister: Option<String>,
    pub required_sandbox_config_hash: Option<String>,
    pub require_landlock: bool,
    pub require_seccomp: bool,
}

/// Settlement status after attestation submission.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SettlementStatus {
    Pending,
    Verified,
    Disputed,
    Settled,
}

/// Response from attestation submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationResponse {
    pub status: SettlementStatus,
    pub message: String,
}

/// Result from atomic payment + attestation (submit_arli_payment).
/// One ICP call = verify + settle + release payment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArliPaymentResult {
    /// Settlement status after atomic call
    pub status: SettlementStatus,
    /// Human-readable message from ENSO
    pub message: String,
    /// Transaction ID on ICP ledger (if payment released)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_id: Option<String>,
    /// Amount released to agent (USDC cents)
    #[serde(default)]
    pub amount_cents: u64,
}

// ============================================================================
// ENSO CONFIGURATION
// ============================================================================

/// Configuration for ENSO integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsoConfig {
    /// ICP gateway URL (e.g., "https://icp0.io" or "http://localhost:4943")
    pub icp_gateway: String,

    /// ENSO Agent Registry canister ID
    pub registry_canister_id: String,

    /// ENSO Contracts canister ID
    pub contracts_canister_id: String,

    /// Path to ICP identity PEM file (for canister calls)
    pub identity_pem_path: Option<String>,

    /// ARLI public key (hex encoded) registered with ENSO
    pub arli_public_key: String,
}

impl Default for EnsoConfig {
    fn default() -> Self {
        Self {
            icp_gateway: "https://icp0.io".into(),
            registry_canister_id: String::new(),
            contracts_canister_id: String::new(),
            identity_pem_path: None,
            arli_public_key: String::new(),
        }
    }
}

// ============================================================================
// ENSO CLIENT (requires `enso` feature)
// ============================================================================

/// ENSO client for interacting with ICP canisters.
///
/// Only available when the `enso` feature is enabled.
#[cfg(feature = "enso")]
pub struct EnsoClient {
    config: EnsoConfig,
    agent: ic_agent::Agent,
}

#[cfg(feature = "enso")]
impl EnsoClient {
    /// Create a new ENSO client connected to ICP.
    pub async fn new(config: EnsoConfig) -> Result<Self, String> {
        let agent = if let Some(ref pem_path) = config.identity_pem_path {
            let identity = ic_agent::identity::BasicIdentity::from_pem_file(pem_path)
                .map_err(|e| format!("load ICP identity: {}", e))?;
            ic_agent::Agent::builder()
                .with_url(&config.icp_gateway)
                .with_identity(identity)
                .build()
                .map_err(|e| format!("build ICP agent: {}", e))?
        } else {
            ic_agent::Agent::builder()
                .with_url(&config.icp_gateway)
                .build()
                .map_err(|e| format!("build ICP agent: {}", e))?
        };

        agent
            .fetch_root_key()
            .await
            .map_err(|e| format!("fetch root key: {}", e))?;

        Ok(Self { config, agent })
    }

    /// Register the ARLI public key with ENSO Registry.
    pub async fn register_public_key(&self, agent_id: &str) -> Result<(), String> {
        let canister_id = self
            .config
            .registry_canister_id
            .parse::<ic_agent::Principal>()
            .map_err(|e| format!("parse canister id: {}", e))?;

        let args = candid::encode_args((agent_id.to_string(), self.config.arli_public_key.clone()))
            .map_err(|e| format!("encode args: {}", e))?;

        self.agent
            .update(&canister_id, "register_arli_key")
            .with_arg(args)
            .call_and_wait()
            .await
            .map_err(|e| format!("call register_arli_key: {}", e))?;

        Ok(())
    }

    /// Submit an ARLI attestation to ENSO Contracts for settlement.
    pub async fn submit_attestation(
        &self,
        attestation: &ArliAttestation,
    ) -> Result<AttestationResponse, String> {
        let canister_id = self
            .config
            .contracts_canister_id
            .parse::<ic_agent::Principal>()
            .map_err(|e| format!("parse canister id: {}", e))?;

        // Serialize attestation to candid-compatible format
        let attestation_json =
            serde_json::to_vec(attestation).map_err(|e| format!("serialize attestation: {}", e))?;
        let attestation_str =
            String::from_utf8(attestation_json).map_err(|e| format!("UTF-8: {}", e))?;

        let args =
            candid::encode_args((attestation_str,)).map_err(|e| format!("encode args: {}", e))?;

        let result = self
            .agent
            .update(&canister_id, "submit_arli_attestation")
            .with_arg(args)
            .call_and_wait()
            .await
            .map_err(|e| format!("call submit_arli_attestation: {}", e))?;

        // Decode response — expect Result variant
        let response_str: String = candid::decode_args(&result)
            .map_err(|e| format!("decode response: {}", e))?
            .0;

        serde_json::from_str(&response_str).map_err(|e| format!("parse response: {}", e))
    }

    /// Submit attestation AND trigger atomic payment settlement.
    ///
    /// One ICP call: verify attestation → settle contract → release escrowed payment.
    /// Uses ENSO's `submit_arli_payment` endpoint (P0 — replaces Ethereum x402).
    pub async fn submit_arli_payment(
        &self,
        contract_id: &str,
        attestation_json: &str,
    ) -> Result<ArliPaymentResult, String> {
        let canister_id = self
            .config
            .contracts_canister_id
            .parse::<ic_agent::Principal>()
            .map_err(|e| format!("parse canister id: {}", e))?;

        let args = candid::encode_args((contract_id.to_string(), attestation_json.to_string()))
            .map_err(|e| format!("encode args: {}", e))?;

        let result = self
            .agent
            .update(&canister_id, "submit_arli_payment")
            .with_arg(args)
            .call_and_wait()
            .await
            .map_err(|e| format!("call submit_arli_payment: {}", e))?;

        // Decode response — Candid variant { Ok: ArliPaymentResult; Err: text }
        let response_str: String = candid::decode_args(&result)
            .map_err(|e| format!("decode response: {}", e))?
            .0;

        serde_json::from_str(&response_str).map_err(|e| format!("parse payment result: {}", e))
    }
}

/// Stub client when `enso` feature is not enabled.
#[cfg(not(feature = "enso"))]
pub struct EnsoClientStub;

#[cfg(not(feature = "enso"))]
impl EnsoClientStub {
    pub fn new(_config: EnsoConfig) -> Self {
        Self
    }

    pub async fn register_public_key(&self, _agent_id: &str) -> Result<(), String> {
        Err("ENSO integration not compiled — rebuild with `--features enso`".into())
    }

    pub async fn submit_attestation(
        &self,
        _attestation: &ArliAttestation,
    ) -> Result<AttestationResponse, String> {
        Err("ENSO integration not compiled — rebuild with `--features enso`".into())
    }

    pub async fn submit_arli_payment(
        &self,
        _contract_id: &str,
        _attestation_json: &str,
    ) -> Result<ArliPaymentResult, String> {
        Err("ENSO integration not compiled — rebuild with `--features enso`".into())
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enso_config_default() {
        let config = EnsoConfig::default();
        assert_eq!(config.icp_gateway, "https://icp0.io");
        assert!(config.registry_canister_id.is_empty());
    }

    #[test]
    fn test_trust_model_variants() {
        let m = TrustModel::KernelSandbox;
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("KernelSandbox"));
    }

    #[test]
    fn test_agent_registration_serialization() {
        let reg = EnsoAgentRegistration {
            name: "test-agent".into(),
            description: "Test".into(),
            version: "1.0".into(),
            base_model: "deepseek-chat".into(),
            system_prompt_hash: "abc123".into(),
            tool_permissions: vec!["api:stripe".into()],
            capabilities: vec![],
            trust_model: TrustModel::KernelSandbox,
            endpoints: EnsoEndpoints {
                a2a_endpoint: None,
                mcp_endpoint: None,
                http_endpoint: Some("https://arli.example.com/agent".into()),
            },
            wallet_addresses: vec![],
        };

        let json = serde_json::to_string(&reg).unwrap();
        assert!(json.contains("KernelSandbox"));
        assert!(json.contains("https://arli.example.com/agent"));
    }

    #[test]
    fn test_sla_metric_defaults() {
        let sla = SlaMetric {
            name: "sandbox".into(),
            target: "landlock+seccomp".into(),
            verifier_canister: None,
            required_sandbox_config_hash: None,
            require_landlock: false,
            require_seccomp: false,
        };
        assert_eq!(sla.name, "sandbox");
        assert!(!sla.require_landlock);
    }

    #[test]
    fn test_settlement_status() {
        assert_eq!(
            serde_json::to_string(&SettlementStatus::Verified).unwrap(),
            "\"Verified\""
        );
    }

    #[test]
    fn test_arli_payment_result() {
        let r = ArliPaymentResult {
            status: SettlementStatus::Verified,
            message: "Payment released".into(),
            tx_id: Some("0xabc123".into()),
            amount_cents: 5000,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("Verified"));
        assert!(json.contains("0xabc123"));
        assert!(json.contains("5000"));

        // Without tx_id (error case)
        let r2 = ArliPaymentResult {
            status: SettlementStatus::Disputed,
            message: "Failed".into(),
            tx_id: None,
            amount_cents: 0,
        };
        let json2 = serde_json::to_string(&r2).unwrap();
        assert!(json2.contains("Disputed"));
        assert!(!json2.contains("tx_id"));
    }
}
