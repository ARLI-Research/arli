//! Agent Profile — Internet Identity-based agent registration.
//!
//! Flow:
//!   1. User runs `arli setup --ii`
//!   2. Generates ephemeral keypair → constructs II auth URL
//!   3. Opens browser → II authenticates via WebAuthn
//!   4. II redirects back to localhost with delegation chain
//!   5. CLI parses delegation, creates ic-agent Identity
//!   6. Pulls/creates AgentProfile on-chain
//!   7. Auto-registers with ENSO
//!   8. Saves config to ~/.arli/

#[cfg(feature = "enso")]
use crate::attestation::ArliKeypair;
#[cfg(feature = "enso")]
use ed25519_dalek::SigningKey;
#[cfg(feature = "enso")]
use ic_agent::identity::Delegation;
#[cfg(feature = "enso")]
use ic_agent::Identity;
#[cfg(feature = "enso")]
use rand::rngs::OsRng;
#[cfg(feature = "enso")]
use serde::Deserialize;
#[cfg(feature = "enso")]
use std::time::{SystemTime, UNIX_EPOCH};

// ── II Auth Flow ────────────────────────────────────────

/// II authentication URL builder.
pub struct IiAuthFlow {
    /// Ephemeral session key (ed25519)
    pub session_key: SigningKey,
    /// Local callback port
    pub port: u16,
    /// II canister ID (mainnet)
    pub ii_canister: String,
}

impl IiAuthFlow {
    pub fn new() -> Self {
        let session_key = SigningKey::generate(&mut OsRng);
        Self {
            session_key,
            port: 0, // assigned at start
            ii_canister: "rdmx6-jaaaa-aaaaa-aaadq-cai".into(),
        }
    }

    /// Build the II authorization URL.
    pub fn auth_url(&self) -> String {
        let pubkey_hex = hex::encode(self.session_key.verifying_key().as_bytes());
        format!(
            "https://identity.ic0.app/#authorize\
             ?session_public_key={}\
             &redirect_uri=http://localhost:{}/callback\
             &max_time_to_live=3600000000000", // 1 hour in nanos
            pubkey_hex, self.port
        )
    }

    /// Parse II callback delegation from JSON.
    pub fn parse_delegation(raw: &str) -> Result<DelegationChain, String> {
        let callback: IiCallback =
            serde_json::from_str(raw).map_err(|e| format!("parse II callback: {}", e))?;

        let delegation = base64url_decode(&callback.delegation)
            .map_err(|e| format!("decode delegation: {}", e))?;

        // Parse as Candid-encoded delegation
        let parsed = parse_delegation_candid(&delegation)?;

        Ok(DelegationChain {
            delegation: parsed,
            user_public_key: callback.user_public_key,
        })
    }
}

/// Parsed delegation chain from II.
pub struct DelegationChain {
    pub delegation: Delegation,
    pub user_public_key: Vec<u8>,
}

#[derive(Deserialize)]
struct IiCallback {
    delegation: String,
    user_public_key: Vec<u8>,
    #[serde(default)]
    signature: Option<String>,
}

/// Minimal Candid delegation parser.
/// II encodes delegation as Candid bytes — structure:
///   record { pubkey: blob; expiration: nat64; targets: opt vec principal }
fn parse_delegation_candid(bytes: &[u8]) -> Result<Delegation, String> {
    // II uses a specific Candid encoding for delegations.
    // We parse the raw bytes manually since the structure is simple.
    //
    // Delegation Candid type:
    //   record {
    //     pubkey: blob;
    //     expiration: nat64;
    //     targets: opt vec principal;
    //   }

    if bytes.is_empty() {
        return Err("empty delegation".into());
    }

    // First byte is Dataless opcode
    if bytes[0] != 0xD9 || bytes.len() < 2 {
        return Err("not a delegation Candid record".into());
    }

    // Number of fields (should be 3)
    let num_fields = bytes[1] as usize;
    if num_fields != 3 {
        return Err(format!(
            "expected 3 fields in delegation, got {}",
            num_fields
        ));
    }

    let mut pos = 2;

    // --- Field 0: pubkey (blob) ---
    if pos >= bytes.len() || bytes[pos] != 0x6E {
        return Err("field 0 not a blob".into());
    }
    pos += 1;

    let pubkey_len = read_leb128(&bytes[pos..]).map_err(|e| format!("pubkey len: {}", e))?;
    let (len_size, pubkey_len) = pubkey_len;
    pos += len_size;

    let pubkey = bytes[pos..pos + pubkey_len as usize].to_vec();
    pos += pubkey_len as usize;

    // --- Field 1: expiration (nat64) ---
    if pos >= bytes.len() || bytes[pos] != 0x7C {
        return Err("field 1 not a nat64".into());
    }
    pos += 1;

    let exp =
        u64::from_le_bytes(bytes[pos..pos + 8].try_into().map_err(|_| "exp: not 8 bytes")?);
    pos += 8;

    // --- Field 2: targets (opt vec principal) ---
    // opt: 0x7F 0x01 = Some, 0x7F 0x00 = None
    let targets: Option<Vec<ic_agent::export::Principal>> = if pos < bytes.len() && bytes[pos] == 0x7F
    {
        pos += 1;
        if pos >= bytes.len() {
            return Err("truncated opt".into());
        }

        if bytes[pos] == 0x00 {
            pos += 1;
            None
        } else if bytes[pos] == 0x01 {
            pos += 1;
            // vec length
            let vec_len = read_leb128(&bytes[pos..]).map_err(|e| format!("vec len: {}", e))?;
            let (len_sz, vec_len) = vec_len;
            pos += len_sz;

            let mut principals = Vec::new();
            for _ in 0..vec_len {
                if bytes[pos] != 0x6E {
                    return Err(format!("expected blob at pos {}", pos));
                }
                pos += 1;
                let p_len = read_leb128(&bytes[pos..]).map_err(|e| format!("p len: {}", e))?;
                let (p_len_sz, p_len) = p_len;
                pos += p_len_sz;

                let p_bytes = &bytes[pos..pos + p_len as usize];
                let p = ic_agent::export::Principal::from_slice(p_bytes);
                principals.push(p);
                pos += p_len as usize;
            }
            Some(principals)
        } else {
            return Err(format!("unexpected opt tag: {}", bytes[pos]));
        }
    } else {
        None
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    Ok(Delegation {
        pubkey,
        expiration: exp,
        targets,
    })
}

fn read_leb128(bytes: &[u8]) -> Result<(usize, u64), String> {
    let mut result: u64 = 0;
    let mut shift = 0;
    for (i, &b) in bytes.iter().enumerate() {
        result |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok((i + 1, result));
        }
        shift += 7;
        if shift >= 64 {
            return Err("LEB128 too long".into());
        }
    }
    Err("truncated LEB128".into())
}

fn base64url_decode(input: &str) -> Result<Vec<u8>, String> {
    // Convert base64url → base64, then decode
    let b64 = input.replace('-', "+").replace('_', "/");
    let padding = (4 - (b64.len() % 4)) % 4;
    let padded = b64 + &"=".repeat(padding);

    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(&padded)
        .map_err(|e| format!("base64 decode: {}", e))
}

// ── Agent Profile Client ─────────────────────────────────

/// Client for the AgentProfile canister.
/// Requires an ic-agent Identity (from II auth).
pub struct AgentProfileClient {
    agent: ic_agent::Agent,
    canister_id: ic_agent::export::Principal,
}

impl AgentProfileClient {
    /// Create a new client with the given identity.
    pub async fn new(
        identity: impl Identity + 'static,
        icp_gateway: &str,
        canister_id: &str,
    ) -> Result<Self, String> {
        let agent = ic_agent::Agent::builder()
            .with_url(icp_gateway)
            .with_identity(identity)
            .build()
            .map_err(|e| format!("build agent: {}", e))?;

        // Only fetch root key for local/test networks
        if icp_gateway.contains("localhost") || icp_gateway.contains("127.0.0.1") {
            agent
                .fetch_root_key()
                .await
                .map_err(|e| format!("fetch root key: {}", e))?;
        }

        let canister_id = ic_agent::export::Principal::from_text(canister_id)
            .map_err(|e| format!("parse canister: {}", e))?;

        Ok(Self { agent, canister_id })
    }

    /// Get the caller's profile from the canister.
    pub async fn get_profile(&self) -> Result<Option<AgentProfile>, String> {
        let result = self
            .agent
            .query(&self.canister_id, "get_profile")
            .with_arg(candid::encode_args((Option::<String>::None,)).map_err(|e| format!("encode: {}", e))?)
            .call()
            .await
            .map_err(|e| format!("call get_profile: {}", e))?;

        // Decode as optional profile
        let (profile_opt,): (Option<CandidAgentProfile>,) =
            candid::decode_args(&result).map_err(|e| format!("decode get_profile: {}", e))?;

        Ok(profile_opt.map(|p| p.into()))
    }

    /// Get status for the caller.
    pub async fn get_status(&self) -> Result<ProfileStatus, String> {
        let result = self
            .agent
            .query(&self.canister_id, "get_status")
            .with_arg(candid::encode_args((Option::<String>::None,)).map_err(|e| format!("encode: {}", e))?)
            .call()
            .await
            .map_err(|e| format!("call get_status: {}", e))?;

        let (status,): (CandidProfileStatus,) =
            candid::decode_args(&result).map_err(|e| format!("decode get_status: {}", e))?;

        Ok(status.into())
    }

    /// Create or update profile.
    pub async fn put_profile(
        &self,
        name: &str,
        capabilities: &[String],
    ) -> Result<AgentProfile, String> {
        let result = self
            .agent
            .update(&self.canister_id, "put_profile")
            .with_arg(
                candid::encode_args((name.to_string(), capabilities.to_vec(), Option::<CandidPreferences>::None))
                    .map_err(|e| format!("encode: {}", e))?,
            )
            .call_and_wait()
            .await
            .map_err(|e| format!("call put_profile: {}", e))?;

        let (profile,): (CandidAgentProfile,) =
            candid::decode_args(&result).map_err(|e| format!("decode put_profile: {}", e))?;

        Ok(profile.into())
    }

    /// Register for ENSO attestation.
    pub async fn register_for_enso(
        &self,
        public_key: &str,
        name: &str,
        capabilities: &[String],
    ) -> Result<EnsoRegistrationResult, String> {
        let result = self
            .agent
            .update(&self.canister_id, "register_for_enso")
            .with_arg(
                candid::encode_args((
                    public_key.to_string(),
                    name.to_string(),
                    capabilities.to_vec(),
                ))
                .map_err(|e| format!("encode: {}", e))?,
            )
            .call_and_wait()
            .await
            .map_err(|e| format!("call register_for_enso: {}", e))?;

        let (candid_result,): (CandidRegisterResult,) =
            candid::decode_args(&result).map_err(|e| format!("decode register_for_enso: {}", e))?;

        Ok(candid_result.into())
    }
}

// ── Candid types (Candid requires #[derive(CandidType, Deserialize)]) ──

#[derive(Debug, Clone, candid::CandidType, serde::Deserialize)]
struct CandidAgentProfile {
    principal: candid::Principal,
    name: String,
    enso_registered: bool,
    attestation_pubkey: Option<String>,
    capabilities: Vec<String>,
    preferences: CandidPreferences,
    created_at: candid::Int,
    updated_at: candid::Int,
}

#[derive(Debug, Clone, candid::CandidType, serde::Deserialize)]
struct CandidPreferences {
    model: String,
    max_iterations: u64,
    compression_threshold: f64,
}

#[derive(Debug, Clone, candid::CandidType, serde::Deserialize)]
struct CandidProfileStatus {
    exists: bool,
    enso_registered: bool,
    agent_count: u64,
}

#[derive(Debug, Clone, candid::CandidType, serde::Deserialize)]
enum CandidRegisterResult {
    #[serde(rename = "ok")]
    Ok { agent_id: String },
    #[serde(rename = "err")]
    Err { message: String },
}

// ── Public types (without Candid dependency) ──

#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub principal: String,
    pub name: String,
    pub enso_registered: bool,
    pub attestation_pubkey: Option<String>,
    pub capabilities: Vec<String>,
    pub preferences: Preferences,
}

#[derive(Debug, Clone)]
pub struct Preferences {
    pub model: String,
    pub max_iterations: u64,
    pub compression_threshold: f64,
}

#[derive(Debug, Clone)]
pub struct ProfileStatus {
    pub exists: bool,
    pub enso_registered: bool,
    pub agent_count: u64,
}

#[derive(Debug, Clone)]
pub enum EnsoRegistrationResult {
    Ok { agent_id: String },
    Err { message: String },
}

// ── Conversions ──

impl From<CandidAgentProfile> for AgentProfile {
    fn from(p: CandidAgentProfile) -> Self {
        Self {
            principal: p.principal.to_text(),
            name: p.name,
            enso_registered: p.enso_registered,
            attestation_pubkey: p.attestation_pubkey,
            capabilities: p.capabilities,
            preferences: Preferences {
                model: p.preferences.model,
                max_iterations: p.preferences.max_iterations,
                compression_threshold: p.preferences.compression_threshold,
            },
        }
    }
}

impl From<CandidProfileStatus> for ProfileStatus {
    fn from(s: CandidProfileStatus) -> Self {
        Self {
            exists: s.exists,
            enso_registered: s.enso_registered,
            agent_count: s.agent_count,
        }
    }
}

impl From<CandidRegisterResult> for EnsoRegistrationResult {
    fn from(r: CandidRegisterResult) -> Self {
        match r {
            CandidRegisterResult::Ok { agent_id } => EnsoRegistrationResult::Ok { agent_id },
            CandidRegisterResult::Err { message } => EnsoRegistrationResult::Err { message },
        }
    }
}

// ── II Setup — Parse delegation + call canister ───────────────

#[cfg(feature = "enso")]
use ic_agent::identity::DelegatedIdentity;

/// Complete II setup: parse the delegation from II callback and register on AgentProfile canister.
#[cfg(feature = "enso")]
pub async fn complete_ii_setup(
    delegation_b64: &str,
    user_pubkey_hex: &str,
    session_pubkey_hex: &str,
    session_secret_hex: &str,
    agent_name: &str,
    icp_gateway: &str,
    canister_id: &str,
) -> Result<IiSetupResult, String> {
    use ed25519_dalek::SigningKey;
    use ic_agent::identity::BasicIdentity;

    // 1. Decode delegation from base64 → Candid SignedDelegation
    let delegation_bytes = base64url_decode(delegation_b64)
        .map_err(|e| format!("decode delegation base64: {}", e))?;

    let signed_delegation = parse_signed_delegation(&delegation_bytes)
        .map_err(|e| format!("parse delegation: {}", e))?;

    // 2. Decode user public key (the II anchor's key — DER-encoded)
    let user_pubkey = hex::decode(user_pubkey_hex)
        .map_err(|e| format!("decode user pubkey: {}", e))?;

    // 3. Decode session keypair
    let session_pubkey = hex::decode(session_pubkey_hex)
        .map_err(|e| format!("decode session pubkey: {}", e))?;

    let session_secret = hex::decode(session_secret_hex)
        .map_err(|e| format!("decode session secret: {}", e))?;

    let session_secret: [u8; 32] = session_secret
        .try_into()
        .map_err(|_| "session secret must be 32 bytes".to_string())?;

    let session_key = SigningKey::from_bytes(&session_secret);

    // 4. Create BasicIdentity using ring::Ed25519KeyPair
    let keypair = ring::signature::Ed25519KeyPair::from_seed_and_public_key(
        &session_secret,
        &session_pubkey,
    )
    .map_err(|e| format!("ring keypair: {}", e))?;

    let session_identity = ic_agent::identity::BasicIdentity::from_key_pair(keypair);

    // 5. Create DelegatedIdentity: user → session, with delegation chain
    let identity = DelegatedIdentity::new(
        user_pubkey,
        Box::new(session_identity),
        vec![signed_delegation],
    )
    .map_err(|e| format!("create DelegatedIdentity: {}", e))?;

    // 6. Build ic-agent
    let agent = ic_agent::Agent::builder()
        .with_url(icp_gateway)
        .with_identity(identity)
        .build()
        .map_err(|e| format!("build agent: {}", e))?;

    if icp_gateway.contains("localhost") || icp_gateway.contains("127.0.0.1") {
        agent.fetch_root_key().await.map_err(|e| format!("fetch root key: {}", e))?;
    }

    let canister_principal = ic_agent::export::Principal::from_text(canister_id)
        .map_err(|e| format!("parse canister id: {}", e))?;

    // 7. Create profile if not exists
    let _ = agent
        .update(&canister_principal, "put_profile")
        .with_arg(
            candid::encode_args((
                agent_name.to_string(),
                vec!["attestation".to_string(), "oracle".to_string(), "sandbox".to_string()],
                Option::<(String, u64, f64)>::None,
            ))
            .map_err(|e| format!("encode put_profile: {}", e))?,
        )
        .call_and_wait()
        .await
        .map_err(|e| format!("put_profile: {}", e))?;

    // 8. Register for ENSO
    let enso_result = agent
        .update(&canister_principal, "register_for_enso")
        .with_arg(
            candid::encode_args((
                session_pubkey_hex.to_string(),
                agent_name.to_string(),
                vec!["attestation".to_string(), "oracle".to_string(), "sandbox".to_string()],
            ))
            .map_err(|e| format!("encode register_for_enso: {}", e))?,
        )
        .call_and_wait()
        .await
        .map_err(|e| format!("register_for_enso: {}", e))?;

    let (candid_result,): (CandidRegisterResult,) =
        candid::decode_args(&enso_result).map_err(|e| format!("decode: {}", e))?;

    let agent_id = match candid_result {
        CandidRegisterResult::Ok { agent_id } => agent_id,
        CandidRegisterResult::Err { message } => return Err(message),
    };

    Ok(IiSetupResult {
        agent_id,
        pubkey_hex: session_pubkey_hex.to_string(),
    })
}

/// Parse a Candid-encoded SignedDelegation from raw bytes.
#[cfg(feature = "enso")]
fn parse_signed_delegation(bytes: &[u8]) -> Result<ic_agent::identity::SignedDelegation, String> {
    use ic_agent::identity::{Delegation, SignedDelegation};

    if bytes.is_empty() {
        return Err("empty delegation".into());
    }

    // Candid record(2) = D9 D9 02 ...
    if bytes[0] != 0xD9 || bytes.len() < 3 {
        return Err("not a Candid record".into());
    }

    let num_fields = bytes[1] as usize;
    if num_fields != 2 {
        return Err(format!("expected 2 fields, got {}", num_fields));
    }

    let mut pos = 2;

    // Field 0: delegation (record)
    if bytes[pos] != 0xD9 {
        return Err("field 0 not record".into());
    }
    pos += 1;
    let inner_fields = bytes[pos] as usize;
    pos += 1;
    if inner_fields != 3 {
        return Err(format!("delegation: expected 3 fields, got {}", inner_fields));
    }

    // delegation.pubkey (blob)
    if bytes[pos] != 0x6E { return Err("pubkey not blob".into()); }
    pos += 1;
    let (sz, pk_len) = read_leb128(&bytes[pos..])?;
    pos += sz;
    let pubkey = bytes[pos..pos + pk_len as usize].to_vec();
    pos += pk_len as usize;

    // delegation.expiration (nat64)
    if bytes[pos] != 0x7C { return Err("exp not nat64".into()); }
    pos += 1;
    let exp = u64::from_le_bytes(bytes[pos..pos+8].try_into().map_err(|_| "exp: need 8 bytes")?);
    pos += 8;

    // delegation.targets (opt vec principal)
    let targets = if pos < bytes.len() && bytes[pos] == 0x7F {
        pos += 1;
        match bytes[pos] {
            0x00 => { pos += 1; None }
            0x01 => {
                pos += 1;
                let (sz, n) = read_leb128(&bytes[pos..])?;
                pos += sz;
                let mut principals = Vec::new();
                for _ in 0..n {
                    if bytes[pos] != 0x6E { return Err("target not blob".into()); }
                    pos += 1;
                    let (sz2, plen) = read_leb128(&bytes[pos..])?;
                    pos += sz2;
                    principals.push(ic_agent::export::Principal::from_slice(&bytes[pos..pos+plen as usize]));
                    pos += plen as usize;
                }
                Some(principals)
            }
            _ => return Err(format!("bad opt tag: {}", bytes[pos]))
        }
    } else {
        None
    };

    let delegation = Delegation { pubkey, expiration: exp, targets };

    // Field 1: signature (blob)
    if pos >= bytes.len() || bytes[pos] != 0x6E {
        return Err("sig not blob".into());
    }
    pos += 1;
    let (sz, sig_len) = read_leb128(&bytes[pos..])?;
    pos += sz;
    let signature = bytes[pos..pos + sig_len as usize].to_vec();

    Ok(SignedDelegation { delegation, signature })
}

pub struct IiSetupResult {
    pub agent_id: String,
    pub pubkey_hex: String,
}
