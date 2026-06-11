//! ARLI Trading — Hyperliquid integration via hypersdk.
//!
//! Provides trading tools as ARLI Tool implementations,
//! trading skill contracts (TOML), and agent configurations
//! optimized for algorithmic trading.

pub mod agent;
pub mod client;
pub mod execution;
pub mod handler;
pub mod risk;
pub mod skills;
pub mod strategies;
pub mod strategy;
pub mod tools;

/// Re-export for convenience
pub use skills::TradingSkillRegistry;
pub use strategy::{Direction, MarketSnapshot, Signal, SignalAction, Strategy, StrategyRegistry};
pub use tools::register_trading_tools;
