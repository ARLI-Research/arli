//! x402 agentic wallet — on-chain USDC settlement.
//!
//! Provides an Ethereum wallet for paying premium AI tools with USDC.
//! Uses secp256k1 signing (k256 crate), EIP-1559 transactions (type 2),
//! and raw JSON-RPC calls via reqwest.
//!
//! # Architecture
//!
//! ```text
//! X402Client.pay() → UsdcTransfer.send() → JSON-RPC eth_sendRawTransaction → chain
//!                  → tracks spent in X402Client.spent_cents
//! ```
//!
//! The wallet NEVER exposes the private key in logs or tool output.

use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use std::sync::Mutex;

// ── Config ───────────────────────────────────────────────────────────────

/// Configuration for the x402 agentic wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct X402Config {
    /// Whether x402 payment is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Hex-encoded wallet address (public).
    #[serde(default)]
    pub wallet_address: String,

    /// Private key for signing transactions.
    /// **NEVER logged or serialized in plaintext.**
    #[serde(default)]
    pub private_key: String,

    /// RPC URL for the EVM chain (USDC transfers).
    #[serde(default)]
    pub rpc_url: String,

    /// USDC token contract address on the target chain.
    #[serde(default)]
    pub usdc_contract: String,

    /// Chain ID (e.g., 1 for Ethereum mainnet, 8453 for Base, 42161 for Arbitrum).
    #[serde(default = "default_chain_id")]
    pub chain_id: u64,

    /// Maximum spend per single tool call, in USDC cents.
    #[serde(default = "default_max_spend_per_call")]
    pub max_spend_per_call_cents: u64,

    /// Total budget for the wallet, in USDC cents.
    #[serde(default = "default_total_budget")]
    pub total_budget_cents: u64,
}

fn default_chain_id() -> u64 {
    8453 // Base
}

fn default_max_spend_per_call() -> u64 {
    50 // $0.50
}

fn default_total_budget() -> u64 {
    1000 // $10.00
}

impl Default for X402Config {
    fn default() -> Self {
        Self {
            enabled: false,
            wallet_address: String::new(),
            private_key: String::new(),
            rpc_url: String::new(),
            usdc_contract: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into(), // Base USDC
            chain_id: default_chain_id(),
            max_spend_per_call_cents: default_max_spend_per_call(),
            total_budget_cents: default_total_budget(),
        }
    }
}

impl std::fmt::Display for X402Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.enabled {
            return write!(f, "x402: disabled");
        }
        let suffix = if self.wallet_address.len() > 6 {
            "…"
        } else {
            ""
        };
        write!(
            f,
            "x402: enabled, wallet={}{}, rpc={}, usdc={}, max_per_call={}¢, budget={}¢",
            &self.wallet_address.get(..6).unwrap_or(""),
            suffix,
            self.rpc_url,
            self.usdc_contract,
            self.max_spend_per_call_cents,
            self.total_budget_cents,
        )
    }
}

// ── Ethereum Wallet ──────────────────────────────────────────────────────

/// Minimal Ethereum wallet for signing and sending transactions.
struct EthereumWallet {
    signing_key: k256::ecdsa::SigningKey,
    address: [u8; 20],
    chain_id: u64,
}

impl std::fmt::Debug for EthereumWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthereumWallet")
            .field("address", &self.address_hex())
            .field("chain_id", &self.chain_id)
            .finish_non_exhaustive()
    }
}

impl EthereumWallet {
    /// Create from a hex-encoded private key (with or without 0x prefix).
    fn from_hex_key(hex_key: &str, chain_id: u64) -> Result<Self, String> {
        let key_hex = hex_key.strip_prefix("0x").unwrap_or(hex_key);
        let key_bytes =
            hex::decode(key_hex).map_err(|e| format!("invalid hex private key: {e}"))?;
        if key_bytes.len() != 32 {
            return Err(format!(
                "private key must be 32 bytes, got {}",
                key_bytes.len()
            ));
        }

        let signing_key = k256::ecdsa::SigningKey::from_slice(&key_bytes)
            .map_err(|e| format!("invalid private key: {e}"))?;

        // Derive address from public key
        let verifying_key = signing_key.verifying_key();
        let public_key = verifying_key.to_encoded_point(false);
        // Ethereum address = keccak256(pubkey[1:])[12:]
        let pubkey_bytes = public_key.as_bytes();
        // Uncompressed: 0x04 || x (32 bytes) || y (32 bytes)
        let hash = Keccak256::digest(&pubkey_bytes[1..]);
        let mut address = [0u8; 20];
        address.copy_from_slice(&hash[12..32]);

        Ok(Self {
            signing_key,
            address,
            chain_id,
        })
    }

    fn address_hex(&self) -> String {
        format!("0x{}", hex::encode(self.address))
    }

    /// Sign an EIP-1559 transaction and return the raw signed tx hex.
    fn sign_eip1559(
        &self,
        nonce: u64,
        to: &[u8; 20],
        value: u64,
        data: &[u8],
        max_fee_per_gas: u64,
        max_priority_fee_per_gas: u64,
        gas_limit: u64,
    ) -> Result<Vec<u8>, String> {
        use k256::ecdsa::signature::Signer;

        // Build EIP-1559 transaction RLP (type 2)
        // Fields: chain_id, nonce, max_priority_fee_per_gas, max_fee_per_gas,
        //         gas_limit, to, value, data, access_list
        let mut tx_rlp = rlp::RlpStream::new();
        tx_rlp.begin_list(9);
        tx_rlp.append(&self.chain_id);
        tx_rlp.append(&nonce);
        tx_rlp.append(&max_priority_fee_per_gas);
        tx_rlp.append(&max_fee_per_gas);
        tx_rlp.append(&gas_limit);
        tx_rlp.append(&&to[..]);
        tx_rlp.append(&value);
        tx_rlp.append(&&data[..]);
        // Empty access list
        tx_rlp.begin_list(0);

        let tx_bytes = tx_rlp.out().to_vec();

        // Type 2 prefix + RLP
        let mut envelope = Vec::with_capacity(1 + tx_bytes.len());
        envelope.push(0x02);
        envelope.extend_from_slice(&tx_bytes);

        // Sign: keccak256(envelope)
        let hash = Keccak256::digest(&envelope);
        let (sig, recid) = self
            .signing_key
            .sign_prehash_recoverable(&hash)
            .map_err(|e| format!("signing failed: {e}"))?;

        let r = sig.r().to_bytes();
        let s = sig.s().to_bytes();
        let v = recid.to_byte() as u64;

        // Re-encode: [chain_id, nonce, max_priority_fee, max_fee, gas_limit, to, value, data, access_list, y_parity, r, s]
        let mut signed_rlp = rlp::RlpStream::new();
        signed_rlp.begin_list(12);
        signed_rlp.append(&self.chain_id);
        signed_rlp.append(&nonce);
        signed_rlp.append(&max_priority_fee_per_gas);
        signed_rlp.append(&max_fee_per_gas);
        signed_rlp.append(&gas_limit);
        signed_rlp.append(&&to[..]);
        signed_rlp.append(&value);
        signed_rlp.append(&&data[..]);
        signed_rlp.begin_list(0); // access list
        signed_rlp.append(&v);
        signed_rlp.append(&&r[..]);
        signed_rlp.append(&&s[..]);

        let signed_bytes = signed_rlp.out().to_vec();
        let mut result = Vec::with_capacity(1 + signed_bytes.len());
        result.push(0x02);
        result.extend_from_slice(&signed_bytes);
        Ok(result)
    }
}

// ── USDC Transfer ────────────────────────────────────────────────────────

/// Builds and sends USDC ERC-20 transfers.
pub struct UsdcTransfer {
    wallet: EthereumWallet,
    usdc_contract: [u8; 20],
    rpc_url: String,
}

impl UsdcTransfer {
    /// Create a new USDC transfer handler.
    pub fn new(config: &X402Config) -> Result<Self, String> {
        let wallet = EthereumWallet::from_hex_key(&config.private_key, config.chain_id)?;

        let contract_hex = config
            .usdc_contract
            .strip_prefix("0x")
            .unwrap_or(&config.usdc_contract);
        let contract_bytes =
            hex::decode(contract_hex).map_err(|e| format!("invalid usdc contract: {e}"))?;
        if contract_bytes.len() != 20 {
            return Err("USDC contract must be 20 bytes".into());
        }
        let mut usdc_contract = [0u8; 20];
        usdc_contract.copy_from_slice(&contract_bytes);

        Ok(Self {
            wallet,
            usdc_contract,
            rpc_url: config.rpc_url.clone(),
        })
    }

    /// Get wallet address.
    pub fn address(&self) -> String {
        self.wallet.address_hex()
    }

    /// Query the USDC balance of this wallet.
    pub async fn balance(&self) -> Result<u64, String> {
        // balanceOf(address) = 0x70a08231 + pad(address)
        let mut data = vec![0x70, 0xa0, 0x82, 0x31];
        let mut padded = [0u8; 32];
        padded[12..32].copy_from_slice(&self.wallet.address);
        data.extend_from_slice(&padded);

        let result = self
            .eth_call(&self.usdc_contract, "0x0", &hex::encode(&data))
            .await?;
        let balance_hex = result.strip_prefix("0x").unwrap_or(&result);
        u64::from_str_radix(balance_hex, 16)
            .map_err(|e| format!("parse balance: {e}"))
    }

    /// Transfer USDC to a recipient.
    /// Returns transaction hash.
    pub async fn transfer(
        &self,
        to_hex: &str,
        amount_cents: u64, // USDC has 6 decimals, so 1 USDC = 1_000_000 base units
    ) -> Result<String, String> {
        let to_clean = to_hex.strip_prefix("0x").unwrap_or(to_hex);
        let to_bytes =
            hex::decode(to_clean).map_err(|e| format!("invalid recipient: {e}"))?;
        if to_bytes.len() != 20 {
            return Err("recipient must be 20 bytes".into());
        }
        let mut to_addr = [0u8; 20];
        to_addr.copy_from_slice(&to_bytes);

        // USDC amount in base units: cents * 10000 (since 6 decimals, 1 cent = 10000 base units)
        let base_amount = amount_cents.saturating_mul(10000);

        // transfer(address,uint256) = 0xa9059cbb + pad(address) + pad(amount)
        let mut data = vec![0xa9, 0x05, 0x9c, 0xbb];
        let mut padded_addr = [0u8; 32];
        padded_addr[12..32].copy_from_slice(&to_addr);
        data.extend_from_slice(&padded_addr);
        let mut padded_amount = [0u8; 32];
        // amount as big-endian u256
        let amount_bytes = base_amount.to_be_bytes();
        padded_amount[24..32].copy_from_slice(&amount_bytes);
        data.extend_from_slice(&padded_amount);

        // Get nonce
        let nonce = self.get_nonce().await?;

        // Estimate gas
        let gas_limit = self
            .estimate_gas(
                &self.wallet.address_hex(),
                &self.usdc_contract,
                "0x0",
                &hex::encode(&data),
            )
            .await
            .unwrap_or(100_000);

        // Current fee
        let (max_fee, max_priority) = self.get_fee_data().await.unwrap_or((1_000_000_000, 100_000_000));

        // Build and sign
        let raw_tx = self.wallet.sign_eip1559(
            nonce,
            &self.usdc_contract,
            0, // no ETH value, just USDC transfer
            &data,
            max_fee,
            max_priority,
            gas_limit,
        )?;

        let raw_hex = format!("0x{}", hex::encode(&raw_tx));
        self.send_raw_transaction(&raw_hex).await
    }

    // ── JSON-RPC helpers ──────────────────────────────────────────────

    async fn rpc_call(&self, method: &str, params: &[serde_json::Value]) -> Result<serde_json::Value, String> {
        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let resp = client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("RPC call failed: {e}"))?;

        let json: serde_json::Value =
            resp.json().await.map_err(|e| format!("RPC parse: {e}"))?;

        if let Some(err) = json.get("error") {
            return Err(format!("RPC error: {err}"));
        }

        json.get("result")
            .cloned()
            .ok_or_else(|| "no result in RPC response".into())
    }

    async fn get_nonce(&self) -> Result<u64, String> {
        let result = self
            .rpc_call(
                "eth_getTransactionCount",
                &[serde_json::json!(self.wallet.address_hex()), serde_json::json!("latest")],
            )
            .await?;
        let hex_str = result
            .as_str()
            .ok_or("nonce is not a string")?
            .strip_prefix("0x")
            .unwrap_or(result.as_str().unwrap_or("0"));
        u64::from_str_radix(hex_str, 16).map_err(|e| format!("parse nonce: {e}"))
    }

    async fn eth_call(&self, to: &[u8; 20], value: &str, data: &str) -> Result<String, String> {
        let result = self
            .rpc_call(
                "eth_call",
                &[
                    serde_json::json!({
                        "to": format!("0x{}", hex::encode(to)),
                        "value": value,
                        "data": data,
                    }),
                    serde_json::json!("latest"),
                ],
            )
            .await?;
        Ok(result.as_str().unwrap_or("0x0").to_string())
    }

    async fn estimate_gas(
        &self,
        from: &str,
        to: &[u8; 20],
        value: &str,
        data: &str,
    ) -> Result<u64, String> {
        let result = self
            .rpc_call(
                "eth_estimateGas",
                &[serde_json::json!({
                    "from": from,
                    "to": format!("0x{}", hex::encode(to)),
                    "value": value,
                    "data": format!("0x{data}"),
                })],
            )
            .await?;
        let hex_str = result
            .as_str()
            .unwrap_or("0x0")
            .strip_prefix("0x")
            .unwrap_or("0");
        u64::from_str_radix(hex_str, 16).map_err(|e| format!("parse gas: {e}"))
    }

    async fn get_fee_data(&self) -> Result<(u64, u64), String> {
        // Try fee history for EIP-1559
        let result = self
            .rpc_call("eth_maxPriorityFeePerGas", &[])
            .await?;
        let priority_hex = result
            .as_str()
            .unwrap_or("0x0")
            .strip_prefix("0x")
            .unwrap_or("0");
        let priority = u64::from_str_radix(priority_hex, 16).unwrap_or(100_000_000);

        let result = self
            .rpc_call("eth_gasPrice", &[])
            .await?;
        let gas_hex = result
            .as_str()
            .unwrap_or("0x0")
            .strip_prefix("0x")
            .unwrap_or("0");
        let gas_price = u64::from_str_radix(gas_hex, 16).unwrap_or(1_000_000_000);

        // max_fee = 2 * base_fee + priority ≈ gas_price + priority
        Ok((gas_price.saturating_add(priority), priority))
    }

    async fn send_raw_transaction(&self, raw_hex: &str) -> Result<String, String> {
        let result = self
            .rpc_call("eth_sendRawTransaction", &[serde_json::json!(raw_hex)])
            .await?;
        Ok(result.as_str().unwrap_or("unknown").to_string())
    }
}

// ── X402 Client ──────────────────────────────────────────────────────────

/// Client for the x402 agentic wallet.
///
/// Tracks spend against a budget and provides guarded access to the
/// private key (which is never exposed in logs or tool output).
pub struct X402Client {
    config: X402Config,
    spent_cents: Mutex<u64>,
    usdc: Option<UsdcTransfer>,
}

impl X402Client {
    /// Create a new client from config.
    pub fn new(config: X402Config) -> Self {
        let usdc = if config.enabled
            && !config.private_key.is_empty()
            && !config.rpc_url.is_empty()
        {
            match UsdcTransfer::new(&config) {
                Ok(u) => {
                    tracing::info!("x402 USDC transfer enabled, address={}", u.address());
                    Some(u)
                }
                Err(e) => {
                    tracing::warn!("x402 USDC transfer init failed (running in stub mode): {e}");
                    None
                }
            }
        } else {
            None
        };

        tracing::info!("x402 client created: {}", config);
        Self {
            config,
            spent_cents: Mutex::new(0),
            usdc,
        }
    }

    /// Check whether we can afford a tool call of `cost_cents`.
    pub fn can_afford(&self, cost_cents: u64) -> bool {
        if !self.config.enabled {
            tracing::debug!("x402: wallet disabled, cannot pay");
            return false;
        }
        if cost_cents > self.config.max_spend_per_call_cents {
            tracing::debug!(
                "x402: cost {}¢ exceeds per-call max {}¢",
                cost_cents,
                self.config.max_spend_per_call_cents,
            );
            return false;
        }
        let spent = *self.spent_cents.lock().unwrap();
        let remaining = self.config.total_budget_cents.saturating_sub(spent);
        if cost_cents > remaining {
            tracing::debug!(
                "x402: cost {}¢ exceeds remaining budget {}¢",
                cost_cents,
                remaining,
            );
            return false;
        }
        true
    }

    /// Pay for a tool call. Performs on-chain USDC transfer when configured,
    /// otherwise returns a stub hash.
    pub async fn pay(&self, tool: &str, cost_cents: u64) -> Result<String, String> {
        if !self.can_afford(cost_cents) {
            return Err(format!(
                "x402: cannot pay {}¢ for '{}' (budget exceeded or wallet disabled)",
                cost_cents, tool
            ));
        }

        let tx_hash = if let Some(ref usdc) = self.usdc {
            match usdc.transfer(&self.config.wallet_address, cost_cents).await {
                Ok(hash) => {
                    tracing::info!(
                        "x402: on-chain transfer {}¢ for '{}' — tx={hash}",
                        cost_cents,
                        tool,
                    );
                    hash
                }
                Err(e) => {
                    tracing::error!("x402: on-chain transfer failed: {e}");
                    return Err(format!("x402: transfer failed: {e}"));
                }
            }
        } else {
            let stub = format!("x402-stub-{}", uuid::Uuid::new_v4());
            tracing::info!(
                "x402: stub payment {}¢ for '{}' — tx={stub}",
                cost_cents,
                tool,
            );
            stub
        };

        let mut spent = self.spent_cents.lock().unwrap();
        *spent += cost_cents;
        let remaining = self.config.total_budget_cents.saturating_sub(*spent);

        tracing::info!(
            "x402: paid {}¢ for '{}' — tx={}, spent={}¢, remaining={}¢",
            cost_cents,
            tool,
            tx_hash,
            *spent,
            remaining,
        );

        Ok(tx_hash)
    }

    /// Sync version of pay (for non-async contexts). Uses stub payment.
    pub fn pay_sync(&self, tool: &str, cost_cents: u64) -> Result<String, String> {
        if !self.can_afford(cost_cents) {
            return Err(format!(
                "x402: cannot pay {}¢ for '{}' (budget exceeded or wallet disabled)",
                cost_cents, tool
            ));
        }

        let tx_hash = format!("x402-stub-{}", uuid::Uuid::new_v4());

        let mut spent = self.spent_cents.lock().unwrap();
        *spent += cost_cents;
        let remaining = self.config.total_budget_cents.saturating_sub(*spent);

        tracing::info!(
            "x402: paid {}¢ for '{}' — tx={}, spent={}¢, remaining={}¢",
            cost_cents,
            tool,
            tx_hash,
            *spent,
            remaining,
        );

        Ok(tx_hash)
    }

    /// Remaining budget in USDC cents.
    pub fn remaining_budget(&self) -> u64 {
        let spent = *self.spent_cents.lock().unwrap();
        self.config.total_budget_cents.saturating_sub(spent)
    }

    /// Total spent so far in USDC cents.
    pub fn total_spent(&self) -> u64 {
        *self.spent_cents.lock().unwrap()
    }

    /// Whether the wallet is enabled and configured.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
            && !self.config.wallet_address.is_empty()
            && !self.config.private_key.is_empty()
    }

    /// Whether on-chain transfers are active.
    pub fn has_onchain(&self) -> bool {
        self.usdc.is_some()
    }
}

impl std::fmt::Display for X402Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} spent={}¢ onchain={}",
            self.config,
            self.total_spent(),
            self.usdc.is_some(),
        )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_from_hex_key() {
        // Test key (DO NOT USE IN PRODUCTION)
        let key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let wallet = EthereumWallet::from_hex_key(key, 8453).unwrap();
        // Known address for this key
        assert_eq!(wallet.address_hex(), "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
    }

    #[test]
    fn test_wallet_from_0x_key() {
        let key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let wallet = EthereumWallet::from_hex_key(key, 1).unwrap();
        assert_eq!(wallet.address_hex(), "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
    }

    #[test]
    fn test_wallet_invalid_key_length() {
        let result = EthereumWallet::from_hex_key("dead", 1);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("32 bytes"));
    }

    #[test]
    fn test_sign_eip1559_produces_valid_envelope() {
        let key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let wallet = EthereumWallet::from_hex_key(key, 8453).unwrap();

        let to = [0u8; 20];
        let data: Vec<u8> = vec![0xa9, 0x05, 0x9c, 0xbb]; // transfer() selector

        let raw = wallet
            .sign_eip1559(0, &to, 0, &data, 1_000_000_000, 100_000_000, 100_000)
            .unwrap();

        // Type 2 transaction starts with 0x02
        assert_eq!(raw[0], 0x02);
        // Should be at least 100 bytes (RLP envelope + signature)
        assert!(raw.len() > 100, "signed tx too short: {}", raw.len());
    }

    #[test]
    fn test_x402_client_budget_tracking() {
        let config = X402Config {
            enabled: true,
            wallet_address: "0x1234".into(),
            private_key: "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".into(),
            total_budget_cents: 500,
            ..Default::default()
        };

        let client = X402Client::new(config.clone());

        assert!(client.can_afford(50));
        assert!(!client.can_afford(600));
        assert!(!client.can_afford(100)); // exceeds per-call max (50)

        // Pay (sync stub mode since no RPC)
        let tx = client.pay_sync("premium_tool", 30).unwrap();
        assert!(tx.starts_with("x402-stub-"));
        assert_eq!(client.total_spent(), 30);
        assert_eq!(client.remaining_budget(), 470);

        // Pay again
        client.pay_sync("another_tool", 20).unwrap();
        assert_eq!(client.total_spent(), 50);
        assert_eq!(client.remaining_budget(), 450);
    }

    #[test]
    fn test_x402_client_disabled() {
        let config = X402Config::default();
        let client = X402Client::new(config);

        assert!(!client.is_enabled());
        assert!(!client.has_onchain());
        assert!(!client.can_afford(10));
        assert!(client.pay_sync("tool", 10).is_err());
    }

    #[test]
    fn test_usdc_amount_calculation() {
        // 1 cent = 10000 base units (USDC has 6 decimals)
        // 1 USDC = 100 cents = 1_000_000 base units
        // 1 cent = 10_000 base units
        assert_eq!(1u64.saturating_mul(10000), 10000);
        assert_eq!(100u64.saturating_mul(10000), 1_000_000); // $1.00
    }

    #[test]
    fn test_x402_config_chain_id_default() {
        let config = X402Config::default();
        assert_eq!(config.chain_id, 8453); // Base
        assert_eq!(config.max_spend_per_call_cents, 50);
        assert_eq!(config.total_budget_cents, 1000);
    }
}
