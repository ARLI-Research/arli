//! Trading tools — Hyperliquid execution wrapped as ARLI Tools.
//!
//! These tools integrate with hypersdk for order execution, market data,
//! and account management. Each tool validates inputs against skill contracts
//! and enforces safety policies before execution.

use async_trait::async_trait;
use arli_core::tools::{Tool, ToolOutput};

use crate::skills::TradingSkillRegistry;

/// Execute a trade (market or limit order).
pub struct ExecuteTradeTool;

#[async_trait]
impl Tool for ExecuteTradeTool {
    fn name(&self) -> &str {
        "execute_trade"
    }

    fn description(&self) -> &str {
        "Execute a trade on Hyperliquid. Specify coin, side (long/short), \
         size in USD, leverage, and order type (market/limit). \
         CAUTION: This executes real trades. Always verify parameters."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::execute_trade().to_function_schema()
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Invalid JSON arguments: {}", e)),
                }
            }
        };

        let coin = args["coin"].as_str().unwrap_or("unknown");
        let side = args["side"].as_str().unwrap_or("long");
        let size = args["size_usd"].as_f64().unwrap_or(0.0);
        let leverage = args["leverage"].as_u64().unwrap_or(1);
        let order_type = args["order_type"].as_str().unwrap_or("market");

        // Validate against skill contract
        let contract = TradingSkillRegistry::execute_trade();
        if let Err(errors) = contract.validate(&args) {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Validation failed: {}", errors.join("; "))),
            };
        }

        // TODO: Integrate with hypersdk for actual execution
        // For now, return a simulated result
        let order_id = format!("sim-{}", ulid::Ulid::new());

        tracing::info!(
            "TRADE: {} {} ${:.2} on {} (lev {}x) → {}",
            side, coin, size, order_type, leverage, order_id
        );

        let result = serde_json::json!({
            "order_id": order_id,
            "coin": coin,
            "side": side,
            "size_usd": size,
            "leverage": leverage,
            "order_type": order_type,
            "status": "simulated",
            "note": "hypersdk integration pending — this is a simulation"
        });

        ToolOutput {
            success: true,
            content: serde_json::to_string_pretty(&result).unwrap_or_default(),
            error: None,
        }
    }
}

/// Cancel an existing order.
pub struct CancelOrderTool;

#[async_trait]
impl Tool for CancelOrderTool {
    fn name(&self) -> &str {
        "cancel_order"
    }

    fn description(&self) -> &str {
        "Cancel an existing order by ID. Requires the coin symbol and order ID."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::cancel_order().to_function_schema()
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Invalid JSON: {}", e)),
                }
            }
        };

        let order_id = args["order_id"].as_str().unwrap_or("unknown");
        let coin = args["coin"].as_str().unwrap_or("unknown");

        tracing::info!("CANCEL: order {} on {}", order_id, coin);

        ToolOutput {
            success: true,
            content: serde_json::to_string_pretty(&serde_json::json!({
                "cancelled": true,
                "order_id": order_id,
                "coin": coin,
                "status": "simulated"
            })).unwrap_or_default(),
            error: None,
        }
    }
}

/// Get current open positions.
pub struct GetPositionsTool;

#[async_trait]
impl Tool for GetPositionsTool {
    fn name(&self) -> &str {
        "get_positions"
    }

    fn description(&self) -> &str {
        "Get current open positions with unrealized PnL, entry price, and mark price."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::get_positions().to_function_schema()
    }

    async fn execute(&self, _arguments: &str) -> ToolOutput {
        // TODO: Integrate with hypersdk
        ToolOutput {
            success: true,
            content: serde_json::to_string_pretty(&serde_json::json!({
                "positions": [],
                "status": "simulated",
                "note": "hypersdk integration pending"
            })).unwrap_or_default(),
            error: None,
        }
    }
}

/// Get market data for a coin.
pub struct GetMarketDataTool;

#[async_trait]
impl Tool for GetMarketDataTool {
    fn name(&self) -> &str {
        "get_market_data"
    }

    fn description(&self) -> &str {
        "Get real-time market data for a coin: price, funding rate, volume, open interest."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::get_market_data().to_function_schema()
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutput {
                    success: false,
                    content: String::new(),
                    error: Some(format!("Invalid JSON: {}", e)),
                }
            }
        };

        let coin = args["coin"].as_str().unwrap_or("unknown");

        // TODO: Integrate with hypersdk for real data
        ToolOutput {
            success: true,
            content: serde_json::to_string_pretty(&serde_json::json!({
                "coin": coin,
                "mark_price": 0.0,
                "index_price": 0.0,
                "funding_rate": 0.0,
                "open_interest": 0.0,
                "24h_volume": 0.0,
                "status": "simulated",
                "note": "hypersdk integration pending — connect to live data"
            })).unwrap_or_default(),
            error: None,
        }
    }
}

/// Get account information.
pub struct GetAccountInfoTool;

#[async_trait]
impl Tool for GetAccountInfoTool {
    fn name(&self) -> &str {
        "get_account_info"
    }

    fn description(&self) -> &str {
        "Get account info: equity, margin used/free, total PnL, daily PnL, drawdown."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::get_account_info().to_function_schema()
    }

    async fn execute(&self, _arguments: &str) -> ToolOutput {
        // TODO: Integrate with hypersdk
        ToolOutput {
            success: true,
            content: serde_json::to_string_pretty(&serde_json::json!({
                "equity": 0.0,
                "margin_used": 0.0,
                "margin_free": 0.0,
                "total_pnl": 0.0,
                "daily_pnl": 0.0,
                "drawdown_pct": 0.0,
                "status": "simulated",
                "note": "hypersdk integration pending"
            })).unwrap_or_default(),
            error: None,
        }
    }
}

/// Register all trading tools in a tool registry.
pub fn register_trading_tools(registry: &mut arli_core::tools::ToolRegistry) {
    registry.register(Box::new(ExecuteTradeTool));
    registry.register(Box::new(CancelOrderTool));
    registry.register(Box::new(GetPositionsTool));
    registry.register(Box::new(GetMarketDataTool));
    registry.register(Box::new(GetAccountInfoTool));
    tracing::info!("Registered 5 trading tools");
}
