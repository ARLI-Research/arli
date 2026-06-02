//! Hyperliquid client context — initializes from env vars, holds the signer.
//!
//! Reads `HYPERLIQUID_PRIVATE_KEY` (0x-prefixed hex) from environment.
//! The wallet address is derived from the private key.
//! Testnet mode if `HYPERLIQUID_TESTNET=true`.

use anyhow::{Context, Result};
use hypersdk::hypercore::{self, PrivateKeySigner};
use hypersdk::Address;
use std::sync::Arc;

/// Holds the Hyperliquid HTTP client and signer, wrapped for sharing.
#[derive(Clone)]
pub struct HyperliquidContext {
    pub client: Arc<hypercore::http::Client>,
    pub signer: Arc<PrivateKeySigner>,
    pub address: Address,
    pub is_testnet: bool,
}

impl HyperliquidContext {
    /// Initialize from environment variables.
    ///
    /// - `HYPERLIQUID_PRIVATE_KEY` — required, 0x-prefixed hex
    /// - `HYPERLIQUID_TESTNET` — optional, "true" to use testnet
    pub fn from_env() -> Result<Self> {
        let private_key = std::env::var("HYPERLIQUID_PRIVATE_KEY")
            .context("HYPERLIQUID_PRIVATE_KEY not set")?;

        let signer: PrivateKeySigner = private_key
            .parse()
            .context("Invalid HYPERLIQUID_PRIVATE_KEY — must be 0x-prefixed hex")?;

        let address = signer.address();

        let is_testnet = std::env::var("HYPERLIQUID_TESTNET")
            .map(|v| v == "true")
            .unwrap_or(false);

        let client = if is_testnet {
            hypercore::testnet()
        } else {
            hypercore::mainnet()
        };

        tracing::info!(
            "Hyperliquid {} initialized for {}",
            if is_testnet { "testnet" } else { "mainnet" },
            address,
        );

        Ok(Self {
            client: Arc::new(client),
            signer: Arc::new(signer),
            address,
            is_testnet,
        })
    }
}
