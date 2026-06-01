//! x402 agentic wallet infrastructure.
//!
//! x402 is a protocol for paying for premium AI tools with USDC via an
//! agentic wallet. Instead of subscribing to each API individually, you
//! seed a wallet with $5–10 and each tool call costs a few cents.
//!
//! This module is an **infrastructure stub** — it registers the config,
//! client state, and payment tool. Actual on-chain USDC transfer logic
//! is TODO.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

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

    /// RPC URL for the base chain (USDC transfers).
    #[serde(default)]
    pub rpc_url: String,

    /// Maximum spend per single tool call, in USDC cents.
    #[serde(default = "default_max_spend_per_call")]
    pub max_spend_per_call_cents: u64,

    /// Total budget for the wallet, in USDC cents.
    #[serde(default = "default_total_budget")]
    pub total_budget_cents: u64,
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
            "x402: enabled, wallet={}{}, rpc={}, max_per_call={}¢, budget={}¢",
            &self.wallet_address.get(..6).unwrap_or(""),
            suffix,
            self.rpc_url,
            self.max_spend_per_call_cents,
            self.total_budget_cents,
        )
    }
}

/// Client for the x402 agentic wallet.
///
/// Tracks spend against a budget and provides guarded access to the
/// private key (which is never exposed in logs or tool output).
pub struct X402Client {
    config: X402Config,
    spent_cents: Mutex<u64>,
}

impl X402Client {
    /// Create a new client from config.
    pub fn new(config: X402Config) -> Self {
        tracing::info!("x402 client created: {}", config);
        Self {
            config,
            spent_cents: Mutex::new(0),
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

    /// Pay for a tool call.
    ///
    /// Returns a stub transaction hash. Actual on-chain transfer is TODO.
    pub fn pay(&self, tool: &str, cost_cents: u64) -> Result<String, String> {
        if !self.can_afford(cost_cents) {
            return Err(format!(
                "x402: cannot pay {}¢ for '{}' (budget exceeded or wallet disabled)",
                cost_cents, tool
            ));
        }

        let mut spent = self.spent_cents.lock().unwrap();
        *spent += cost_cents;

        let remaining = self.config.total_budget_cents.saturating_sub(*spent);
        let tx_hash = format!("x402-stub-{}", uuid::Uuid::new_v4());

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
}

impl std::fmt::Display for X402Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} spent={}¢", self.config, self.total_spent(),)
    }
}
