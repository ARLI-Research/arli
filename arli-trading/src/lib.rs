//! ARLI Trading — Hyperliquid integration via hypersdk.
//!
//! Provides trading tools as ARLI Tool implementations,
//! trading skill contracts (TOML), and agent configurations
//! optimized for algorithmic trading.

pub mod agent;
pub mod backtest;
pub mod bias_model;
pub mod client;
pub mod confidence_ledger;
pub mod execution;
pub mod fair_value;
pub mod handler;
pub mod indicators;
pub mod multi_agent;
pub mod optimize;
pub mod risk;
pub mod skills;
pub mod strategies;
pub mod strategy;
pub mod tools;

/// Re-export for convenience
pub use skills::TradingSkillRegistry;
pub use strategy::{Direction, MarketSnapshot, Signal, SignalAction, Strategy, StrategyRegistry};
pub use tools::register_trading_tools;
