//! Trading skill contracts — typed, validated, versioned.
//!
//! These TOML-defined contracts are validated at the harness level
//! before any trade is executed. Parameters are type-checked,
//! ranges enforced, and safety rules applied.

use arli_core::skills::{ParameterDef, SafetyConfig, SkillContract};
use std::collections::HashMap;

/// Registry of all trading skill contracts.
pub struct TradingSkillRegistry {
    pub contracts: Vec<SkillContract>,
}

impl TradingSkillRegistry {
    /// Load default trading skills.
    pub fn default_skills() -> Vec<SkillContract> {
        vec![
            Self::execute_trade(),
            Self::cancel_order(),
            Self::get_positions(),
            Self::get_market_data(),
            Self::get_account_info(),
        ]
    }

    /// Execute a market or limit order on Hyperliquid.
    pub fn execute_trade() -> SkillContract {
        let mut params = HashMap::new();
        params.insert(
            "coin".into(),
            ParameterDef {
                param_type: "string".into(),
                description: "Coin symbol (e.g., 'ETH', 'BTC')".into(),
                required: true,
                default: None,
                min: None,
                max: None,
                values: None,
            },
        );
        params.insert(
            "side".into(),
            ParameterDef {
                param_type: "string".into(),
                description: "Trade direction".into(),
                required: true,
                default: None,
                min: None,
                max: None,
                values: Some(vec!["long".into(), "short".into()]),
            },
        );
        params.insert(
            "size_usd".into(),
            ParameterDef {
                param_type: "float".into(),
                description: "Position size in USD".into(),
                required: true,
                default: None,
                min: Some(10.0),
                max: Some(100_000.0),
                values: None,
            },
        );
        params.insert(
            "leverage".into(),
            ParameterDef {
                param_type: "integer".into(),
                description: "Leverage multiplier (1-50)".into(),
                required: false,
                default: Some(serde_json::json!(1)),
                min: Some(1.0),
                max: Some(50.0),
                values: None,
            },
        );
        params.insert(
            "order_type".into(),
            ParameterDef {
                param_type: "string".into(),
                description: "Market or limit order".into(),
                required: false,
                default: Some(serde_json::json!("market")),
                min: None,
                max: None,
                values: Some(vec!["market".into(), "limit".into()]),
            },
        );
        params.insert(
            "limit_price".into(),
            ParameterDef {
                param_type: "float".into(),
                description: "Limit price (required for limit orders)".into(),
                required: false,
                default: None,
                min: Some(0.0),
                max: None,
                values: None,
            },
        );

        SkillContract {
            name: "execute_trade".into(),
            version: "1.0.0".into(),
            description: "Execute a trade on Hyperliquid. Supports market and limit orders with leverage.".into(),
            parameters: params,
            returns: Some(serde_json::json!({
                "order_id": "string",
                "filled": "boolean",
                "avg_price": "float",
                "size": "float"
            })),
            errors: {
                let mut e = HashMap::new();
                e.insert("INSUFFICIENT_MARGIN".into(), "Not enough margin for this trade".into());
                e.insert("MARKET_CLOSED".into(), "Market is currently closed".into());
                e.insert("INVALID_SIZE".into(), "Position size outside allowed range".into());
                e.insert("RATE_LIMITED".into(), "Too many orders — rate limited".into());
                e
            },
            safety: SafetyConfig {
                approval: "always".into(),
                rate_limit: Some("5/minute".into()),
                max_value_usd: Some(10_000.0),
            },
            toolset: "trading".into(),
        }
    }

    /// Cancel an existing order.
    pub fn cancel_order() -> SkillContract {
        let mut params = HashMap::new();
        params.insert(
            "order_id".into(),
            ParameterDef {
                param_type: "string".into(),
                description: "Order ID to cancel".into(),
                required: true,
                default: None,
                min: None,
                max: None,
                values: None,
            },
        );
        params.insert(
            "coin".into(),
            ParameterDef {
                param_type: "string".into(),
                description: "Coin symbol".into(),
                required: true,
                default: None,
                min: None,
                max: None,
                values: None,
            },
        );

        SkillContract {
            name: "cancel_order".into(),
            version: "1.0.0".into(),
            description: "Cancel an existing order on Hyperliquid.".into(),
            parameters: params,
            returns: Some(serde_json::json!({
                "cancelled": "boolean",
                "order_id": "string"
            })),
            errors: {
                let mut e = HashMap::new();
                e.insert("ORDER_NOT_FOUND".into(), "Order ID not found".into());
                e.insert("ALREADY_FILLED".into(), "Order already filled".into());
                e
            },
            safety: SafetyConfig {
                approval: "always".into(),
                rate_limit: Some("10/minute".into()),
                max_value_usd: None,
            },
            toolset: "trading".into(),
        }
    }

    /// Get current open positions.
    pub fn get_positions() -> SkillContract {
        SkillContract {
            name: "get_positions".into(),
            version: "1.0.0".into(),
            description: "Get current open positions and their PnL.".into(),
            parameters: HashMap::new(),
            returns: Some(serde_json::json!({
                "positions": [{
                    "coin": "string",
                    "side": "string",
                    "size": "float",
                    "entry_price": "float",
                    "mark_price": "float",
                    "unrealized_pnl": "float",
                    "leverage": "integer"
                }]
            })),
            errors: HashMap::new(),
            safety: SafetyConfig {
                approval: "never".into(),
                rate_limit: None,
                max_value_usd: None,
            },
            toolset: "trading".into(),
        }
    }

    /// Get market data for a coin.
    pub fn get_market_data() -> SkillContract {
        let mut params = HashMap::new();
        params.insert(
            "coin".into(),
            ParameterDef {
                param_type: "string".into(),
                description: "Coin symbol".into(),
                required: true,
                default: None,
                min: None,
                max: None,
                values: None,
            },
        );

        SkillContract {
            name: "get_market_data".into(),
            version: "1.0.0".into(),
            description: "Get current market data: price, funding rate, open interest.".into(),
            parameters: params,
            returns: Some(serde_json::json!({
                "coin": "string",
                "mark_price": "float",
                "index_price": "float",
                "funding_rate": "float",
                "open_interest": "float",
                "24h_volume": "float",
                "24h_change_pct": "float"
            })),
            errors: {
                let mut e = HashMap::new();
                e.insert("UNKNOWN_COIN".into(), "Coin not found on exchange".into());
                e
            },
            safety: SafetyConfig {
                approval: "never".into(),
                rate_limit: Some("60/minute".into()),
                max_value_usd: None,
            },
            toolset: "trading".into(),
        }
    }

    /// Get account info: margin, equity, PnL.
    pub fn get_account_info() -> SkillContract {
        SkillContract {
            name: "get_account_info".into(),
            version: "1.0.0".into(),
            description: "Get account information: margin, equity, total PnL.".into(),
            parameters: HashMap::new(),
            returns: Some(serde_json::json!({
                "equity": "float",
                "margin_used": "float",
                "margin_free": "float",
                "total_pnl": "float",
                "daily_pnl": "float",
                "drawdown_pct": "float"
            })),
            errors: HashMap::new(),
            safety: SafetyConfig {
                approval: "never".into(),
                rate_limit: None,
                max_value_usd: None,
            },
            toolset: "trading".into(),
        }
    }
}
