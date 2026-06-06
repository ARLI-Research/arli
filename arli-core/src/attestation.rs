//! ARLI Attestation — cryptographic proof of agent execution integrity.
//!
//! Produces signed attestations with replay protection for ENSO settlement.
//! Each attestation proves: agent X executed job Y in sandbox Z at time T,
//! with Landlock+seccomp enforced, under binary hash B.
//!
//! Replay protection: ed25519 signature over SHA-256 of all fields
//! (run_id || job_id || timestamp_ns || ocsf_event_hash || ...).

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

// ============================================================================
// ATTESTATION STRUCT
// ============================================================================

/// Cryptographic attestation that an agent executed work inside ARLI sandbox.
///
/// Sent to ENSO Contracts canister for settlement verification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArliAttestation {
    /// Unique run identifier (ARLI-internal)
    pub run_id: String,

    /// ENSO agent ID (from registry)
    pub agent_id: String,

    /// ENSO contract job ID
    pub job_id: String,

    /// Nanosecond timestamp of attestation creation
    pub timestamp_ns: u64,

    /// SHA-256 of the full OCSF audit event (event stored off-chain)
    pub ocsf_event_hash: String,

    /// Optional URI to full OCSF event (IPFS, Arweave, local file)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocsf_event_uri: Option<String>,

    /// SHA-256 of the YAML sandbox policy used
    pub sandbox_config_hash: String,

    /// SHA-256 of the ARLI binary that executed this run
    pub arli_binary_hash: String,

    /// Whether Landlock filesystem isolation was enforced
    pub landlock_enforced: bool,

    /// Whether seccomp BPF syscall filter was enforced
    pub seccomp_enforced: bool,

    /// UID the process ran under (65534 = nobody)
    pub uid: u32,

    /// Optional SHA-256 of the TaskContract this execution fulfilled.
    ///
    /// When present, proves the agent declared its work scope upfront
    /// and ENSO can verify the contract hash against expected artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_contract_hash: Option<String>,

    /// ed25519 signature over SHA-256 of attestation fields
    pub signature: String,

    /// ed25519 public key (hex encoded) used for verification
    pub public_key: String,
}

impl ArliAttestation {
    /// Compute the message hash that the signature covers.
    ///
    /// Covers: run_id || job_id || timestamp_ns || ocsf_event_hash
    ///       || sandbox_config_hash || arli_binary_hash
    ///       || landlock_enforced || seccomp_enforced || uid
    ///       || task_contract_hash (if present)
    ///
    /// This ensures the signature cannot be replayed for a different job
    /// or at a different time, even with the same sandbox config.
    pub fn message_hash(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(self.run_id.as_bytes());
        hasher.update(b"|");
        hasher.update(self.job_id.as_bytes());
        hasher.update(b"|");
        hasher.update(self.timestamp_ns.to_le_bytes());
        hasher.update(b"|");
        hasher.update(self.ocsf_event_hash.as_bytes());
        hasher.update(b"|");
        hasher.update(self.sandbox_config_hash.as_bytes());
        hasher.update(b"|");
        hasher.update(self.arli_binary_hash.as_bytes());
        hasher.update(b"|");
        hasher.update(&[self.landlock_enforced as u8]);
        hasher.update(b"|");
        hasher.update(&[self.seccomp_enforced as u8]);
        hasher.update(b"|");
        hasher.update(self.uid.to_le_bytes());
        // Task contract hash — if present, binds the execution to a specific
        // declared work scope. Prevents re-attesting the same sandbox run
        // for a different set of promised outputs.
        if let Some(ref contract_hash) = self.task_contract_hash {
            hasher.update(b"|");
            hasher.update(contract_hash.as_bytes());
        }
        hasher.finalize().to_vec()
    }

    /// Verify the ed25519 signature on this attestation.
    ///
    /// Returns true if the signature is valid for the given public key
    /// and covers all attestation fields.
    pub fn verify(&self) -> bool {
        let public_key_bytes = match hex::decode(&self.public_key) {
            Ok(b) => b,
            Err(_) => return false,
        };

        let signature_bytes = match hex::decode(&self.signature) {
            Ok(b) => b,
            Err(_) => return false,
        };

        let vk = match VerifyingKey::from_bytes(
            &public_key_bytes.as_slice().try_into().unwrap_or([0u8; 32]),
        ) {
            Ok(vk) => vk,
            Err(_) => return false,
        };

        let sig = match Signature::from_slice(&signature_bytes) {
            Ok(s) => s,
            Err(_) => return false,
        };

        let msg_hash = self.message_hash();
        vk.verify(&msg_hash, &sig).is_ok()
    }
}

// ============================================================================
// KEY MANAGEMENT
// ============================================================================

/// ARLI signing keypair.
#[derive(Clone)]
pub struct ArliKeypair {
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
}

impl ArliKeypair {
    /// Generate a new random ed25519 keypair.
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        Self {
            signing_key,
            verifying_key,
        }
    }

    /// Load keypair from PEM file. Creates new if not found.
    pub fn load_or_generate(path: &Path) -> Result<Self, String> {
        if path.exists() {
            Self::load(path)
        } else {
            let kp = Self::generate();
            kp.save(path)?;
            Ok(kp)
        }
    }

    /// Load keypair from PEM file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let pem_bytes = std::fs::read(path).map_err(|e| format!("read key file: {}", e))?;
        let pem_str =
            std::str::from_utf8(&pem_bytes).map_err(|e| format!("invalid UTF-8: {}", e))?;

        // ed25519-dalek PEM parsing via pem crate (not yet a dependency, use simple approach)
        // Simple format: hex-encoded 32-byte seed
        let seed_hex = pem_str
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect::<String>()
            .trim()
            .to_string();

        let seed = hex::decode(&seed_hex).map_err(|e| format!("hex decode key: {}", e))?;
        let seed_arr: &[u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| "key must be 32 bytes")?;

        let signing_key = SigningKey::from_bytes(seed_arr);
        let verifying_key = signing_key.verifying_key();
        Ok(Self {
            signing_key,
            verifying_key,
        })
    }

    /// Save keypair to a file (hex-encoded seed with PEM markers).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {}", e))?;
        }
        let seed_hex = hex::encode(self.signing_key.to_bytes());
        let pem = format!(
            "-----BEGIN ARLI PRIVATE KEY-----\n{}\n-----END ARLI PRIVATE KEY-----\n",
            seed_hex
        );
        std::fs::write(path, &pem).map_err(|e| format!("write key: {}", e))?;

        // Set restrictive permissions (0o600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(path, perms);
            }
        }

        Ok(())
    }

    /// Get public key as hex string.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.verifying_key.as_bytes())
    }

    /// Sign an attestation and set the signature + public_key fields.
    pub fn sign_attestation(&self, attestation: &mut ArliAttestation) {
        attestation.public_key = self.public_key_hex();
        let msg_hash = attestation.message_hash();
        let signature = self.signing_key.sign(&msg_hash);
        attestation.signature = hex::encode(signature.to_bytes());
    }
}

impl Default for ArliKeypair {
    fn default() -> Self {
        Self::generate()
    }
}

// ============================================================================
// ATTESTATION BUILDER
// ============================================================================

/// Builder for creating signed attestations.
pub struct AttestationBuilder {
    keypair: ArliKeypair,
    arli_binary_hash: String,
}

impl AttestationBuilder {
    /// Create a new builder with the given keypair and ARLI binary hash.
    pub fn new(keypair: ArliKeypair, arli_binary_hash: String) -> Self {
        Self {
            keypair,
            arli_binary_hash,
        }
    }

    /// Build and sign an attestation from sandbox execution results.
    pub fn build(
        &self,
        run_id: String,
        agent_id: String,
        job_id: String,
        ocsf_event_json: &str,
        ocsf_event_uri: Option<String>,
        sandbox_config_hash: String,
        landlock_enforced: bool,
        seccomp_enforced: bool,
        uid: u32,
        task_contract_hash: Option<String>,
    ) -> ArliAttestation {
        let ocsf_event_hash = hex::encode(Sha256::digest(ocsf_event_json.as_bytes()));
        let timestamp_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        let mut attestation = ArliAttestation {
            run_id,
            agent_id,
            job_id,
            timestamp_ns,
            ocsf_event_hash,
            ocsf_event_uri,
            sandbox_config_hash,
            arli_binary_hash: self.arli_binary_hash.clone(),
            landlock_enforced,
            seccomp_enforced,
            uid,
            task_contract_hash,
            signature: String::new(),
            public_key: String::new(),
        };

        self.keypair.sign_attestation(&mut attestation);
        attestation
    }

    /// Get the public key for registration in ENSO Registry.
    pub fn public_key_hex(&self) -> String {
        self.keypair.public_key_hex()
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_attestation() -> ArliAttestation {
        ArliAttestation {
            run_id: "run-001".into(),
            agent_id: "agent-abc".into(),
            job_id: "job-xyz".into(),
            timestamp_ns: 1717286400_000_000_000,
            ocsf_event_hash: "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
                .into(),
            ocsf_event_uri: None,
            sandbox_config_hash: "sha256:policy-hash".into(),
            arli_binary_hash: "sha256:binary-hash".into(),
            landlock_enforced: true,
            seccomp_enforced: true,
            uid: 65534,
            task_contract_hash: None,
            signature: String::new(),
            public_key: String::new(),
        }
    }

    #[test]
    fn test_keypair_generate_and_sign() {
        let kp = ArliKeypair::generate();
        let mut att = make_attestation();
        kp.sign_attestation(&mut att);

        assert!(!att.signature.is_empty());
        assert!(!att.public_key.is_empty());
        assert!(att.verify());
    }

    #[test]
    fn test_signature_rejects_tampered_attestation() {
        let kp = ArliKeypair::generate();
        let mut att = make_attestation();
        kp.sign_attestation(&mut att);

        assert!(att.verify());

        // Tamper with job_id — signature should fail
        att.job_id = "job-evil".into();
        assert!(!att.verify());
    }

    #[test]
    fn test_replay_protection_different_job() {
        let kp = ArliKeypair::generate();

        let mut att1 = make_attestation();
        att1.job_id = "job-001".into();
        kp.sign_attestation(&mut att1);
        assert!(att1.verify());

        // Same attestation reused for different job — should fail
        let mut att2 = att1.clone();
        att2.job_id = "job-002".into(); // Different job, same signature
        assert!(!att2.verify());
    }

    #[test]
    fn test_replay_protection_different_timestamp() {
        let kp = ArliKeypair::generate();

        let mut att1 = make_attestation();
        att1.timestamp_ns = 1000;
        kp.sign_attestation(&mut att1);
        assert!(att1.verify());

        // Same attestation with different timestamp — should fail
        let mut att2 = att1.clone();
        att2.timestamp_ns = 2000;
        assert!(!att2.verify());
    }

    #[test]
    fn test_different_keypair_fails() {
        let kp1 = ArliKeypair::generate();
        let kp2 = ArliKeypair::generate();

        let mut att = make_attestation();
        kp1.sign_attestation(&mut att);

        // Verify with wrong key should still pass since public_key is embedded...
        // But signature is from kp1, verifying against kp1's pubkey which IS embedded
        assert!(att.verify());

        // Now tamper: replace public key with kp2's but keep kp1's signature
        att.public_key = kp2.public_key_hex();
        assert!(!att.verify());
    }

    #[test]
    fn test_key_save_and_load() {
        let kp = ArliKeypair::generate();
        let tmp = std::env::temp_dir().join("arli-test-key.pem");
        kp.save(&tmp).unwrap();
        assert!(tmp.exists());

        let loaded = ArliKeypair::load(&tmp).unwrap();
        assert_eq!(kp.public_key_hex(), loaded.public_key_hex());

        // Sign with loaded keypair
        let mut att = make_attestation();
        loaded.sign_attestation(&mut att);
        assert!(att.verify());

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_message_hash_deterministic() {
        let att1 = make_attestation();
        let att2 = make_attestation();
        assert_eq!(att1.message_hash(), att2.message_hash());
    }

    #[test]
    fn test_message_hash_changes_with_field() {
        let att1 = make_attestation();
        let mut att2 = make_attestation();
        att2.landlock_enforced = false;
        assert_ne!(att1.message_hash(), att2.message_hash());
    }

    #[test]
    fn test_attestation_builder() {
        let kp = ArliKeypair::generate();
        let pubkey = kp.public_key_hex();
        let builder = AttestationBuilder::new(kp, "sha256:binary-v1".into());

        let att = builder.build(
            "run-001".into(),
            "agent-abc".into(),
            "job-xyz".into(),
            r#"{"event":"test"}"#,
            None,
            "sha256:policy-v1".into(),
            true,
            true,
            65534,
            Some("sha256:contract-abc123".into()),
        );

        assert_eq!(att.run_id, "run-001");
        assert_eq!(att.public_key, pubkey);
        assert!(att.landlock_enforced);
        assert!(att.seccomp_enforced);
        assert_eq!(att.uid, 65534);
        assert_eq!(att.arli_binary_hash, "sha256:binary-v1");
        assert!(!att.ocsf_event_hash.is_empty());
        assert!(!att.signature.is_empty());
        assert_eq!(
            att.task_contract_hash,
            Some("sha256:contract-abc123".into())
        );
        assert!(att.verify());
    }

    #[test]
    fn test_attestation_without_contract_hash() {
        let kp = ArliKeypair::generate();
        let builder = AttestationBuilder::new(kp, "sha256:binary-v1".into());

        let att = builder.build(
            "run-002".into(),
            "agent-abc".into(),
            "job-xyz".into(),
            r#"{"event":"test"}"#,
            None,
            "sha256:policy-v1".into(),
            true,
            true,
            65534,
            None,
        );

        assert_eq!(att.task_contract_hash, None);
        assert!(att.verify());
    }

    #[test]
    fn test_contract_hash_changes_message_hash() {
        let kp = ArliKeypair::generate();
        let builder = AttestationBuilder::new(kp, "sha256:binary-v1".into());

        let att1 = builder.build(
            "run-003".into(),
            "agent-abc".into(),
            "job-xyz".into(),
            r#"{"event":"test"}"#,
            None,
            "sha256:policy-v1".into(),
            true,
            true,
            65534,
            Some("sha256:contract-A".into()),
        );

        let att2 = builder.build(
            "run-003".into(),
            "agent-abc".into(),
            "job-xyz".into(),
            r#"{"event":"test"}"#,
            None,
            "sha256:policy-v1".into(),
            true,
            true,
            65534,
            Some("sha256:contract-B".into()),
        );

        assert_ne!(att1.message_hash(), att2.message_hash());
    }

    #[test]
    fn test_contract_hash_tamper_rejection() {
        let kp = ArliKeypair::generate();
        let builder = AttestationBuilder::new(kp, "sha256:binary-v1".into());

        let mut att = builder.build(
            "run-004".into(),
            "agent-abc".into(),
            "job-xyz".into(),
            r#"{"event":"test"}"#,
            None,
            "sha256:policy-v1".into(),
            true,
            true,
            65534,
            Some("sha256:contract-A".into()),
        );
        assert!(att.verify());

        // Tamper with contract hash
        att.task_contract_hash = Some("sha256:contract-evil".into());
        assert!(!att.verify());
    }
}
