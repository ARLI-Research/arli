//! ARLI Trading — Hyperliquid integration via hypersdk.
//!
//! Provides trading tools as ARLI Tool implementations,
//! trading skill contracts (TOML), and agent configurations
//! optimized for algorithmic trading.

pub mod client;
pub mod skills;
pub mod tools;

/// Re-export for convenience
pub use skills::TradingSkillRegistry;
pub use tools::register_trading_tools;
