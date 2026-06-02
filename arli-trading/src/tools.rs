//! Trading tools — Hyperliquid execution via hypersdk v0.2.12.
//!
//! Each tool uses real API calls through `HyperliquidContext`.
//! Market orders use `market_open()`. Limit orders use `place()`.
//! All orders produce an OCSF event hash for attestation.

use arli_core::tools::{Tool, ToolOutput};
use async_trait::async_trait;
use hypersdk::hypercore::types::*;
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::client::HyperliquidContext;
use crate::skills::TradingSkillRegistry;

fn ocsf_hash(event_json: &serde_json::Value) -> String {
    let json_str = serde_json::to_string(event_json).unwrap_or_default();
    hex::encode(Sha256::digest(json_str.as_bytes()))
}

fn is_buy(side: &str) -> bool {
    matches!(side, "long" | "buy")
}

// ============================================================================
// ExecuteTradeTool
// ============================================================================

pub struct ExecuteTradeTool {
    ctx: Arc<HyperliquidContext>,
}

impl ExecuteTradeTool {
    pub fn new(ctx: Arc<HyperliquidContext>) -> Self { Self { ctx } }
}

#[async_trait]
impl Tool for ExecuteTradeTool {
    fn name(&self) -> &str { "execute_trade" }

    fn description(&self) -> &str {
        "Execute a trade on Hyperliquid. Coin, side (long/short), \
         size USD, leverage (1-50), order_type (market/limit). \
         Limit orders need limit_price. CAUTION: REAL trades."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::execute_trade().to_function_schema()
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Invalid JSON: {}", e)),
            },
        };

        if let Err(errors) = TradingSkillRegistry::execute_trade().validate(&args) {
            return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Validation: {}", errors.join("; "))),
            };
        }

        let coin = args["coin"].as_str().unwrap_or("BTC").to_uppercase();
        let buy = is_buy(args["side"].as_str().unwrap_or("long"));
        let size: Decimal = args["size_usd"].as_f64()
            .and_then(|v| Decimal::try_from(v).ok()).unwrap_or_default();
        let leverage: u64 = args["leverage"].as_u64().unwrap_or(1);
        let order_type = args["order_type"].as_str().unwrap_or("market");
        let limit_price: Option<Decimal> = args.get("limit_price")
            .and_then(|v| v.as_f64()).and_then(|p| Decimal::try_from(p).ok());

        if size <= Decimal::ZERO {
            return ToolOutput {
                success: false, content: String::new(),
                error: Some("size_usd must be > 0".into()),
            };
        }

        // Find market
        let markets = match self.ctx.client.perps().await {
            Ok(m) => m,
            Err(e) => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Failed to fetch markets: {}", e)),
            },
        };

        let market = match markets.iter().find(|m| m.name.eq_ignore_ascii_case(&coin)) {
            Some(m) => m,
            None => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Coin '{}' not found", coin)),
            },
        };

        // Set leverage if needed
        if leverage != 1 {
            let nonce = chrono::Utc::now().timestamp_millis() as u64;
            if let Err(e) = self.ctx.client.update_leverage(
                self.ctx.signer.as_ref(),
                market.index,
                true, // is_cross
                leverage as u32,
                nonce, None, None,
            ).await {
                tracing::warn!("Leverage update failed (may already be set): {}", e);
            }
        }

        let nonce = chrono::Utc::now().timestamp_millis() as u64;

        let result = match order_type {
            "limit" => {
                let batch = BatchOrder {
                    orders: vec![OrderRequest {
                        asset: market.index,
                        is_buy: buy,
                        limit_px: limit_price.unwrap_or(Decimal::ZERO),
                        sz: size,
                        reduce_only: false,
                        order_type: OrderTypePlacement::Limit { tif: TimeInForce::Gtc },
                        cloid: Default::default(),
                    }],
                    grouping: OrderGrouping::Na,
                    builder: None,
                };
                self.ctx.client.place(
                    self.ctx.signer.as_ref(), batch, nonce, None, None,
                ).await.map_err(|e| format!("{:?}", e))
            }
            _ => {
                // market order via market_open
                let worst_price = limit_price.unwrap_or_else(|| {
                    if buy { Decimal::MAX / Decimal::TEN } else { Decimal::ZERO }
                });
                self.ctx.client.market_open(
                    self.ctx.signer.as_ref(),
                    market,
                    buy,
                    worst_price,
                    size,
                    nonce,
                    None, None, None,
                ).await.map_err(|e| e.to_string())
            }
        };

        match result {
            Ok(statuses) => {
                let ocsf = serde_json::json!({
                    "class_uid": 6007,
                    "activity_name": "ExecuteTrade",
                    "coin": coin,
                    "side": if buy { "long" } else { "short" },
                    "size_usd": size.to_string(),
                    "leverage": leverage,
                    "order_type": order_type,
                    "nonce": nonce,
                    "env": if self.ctx.is_testnet { "testnet" } else { "mainnet" },
                });
                let event_hash = ocsf_hash(&ocsf);

                tracing::info!(
                    "TRADE: {} {} ${} on {} ({}x) — ocsf:{}",
                    if buy { "LONG" } else { "SHORT" }, coin,
                    size, order_type, leverage, &event_hash[..16],
                );

                ToolOutput {
                    success: true,
                    content: serde_json::to_string_pretty(&serde_json::json!({
                        "status": "executed",
                        "coin": coin,
                        "side": if buy { "long" } else { "short" },
                        "size_usd": size.to_string(),
                        "leverage": leverage,
                        "order_type": order_type,
                        "nonce": nonce,
                        "ocsf_event_hash": event_hash,
                        "fills": statuses.iter().map(|s| format!("{:?}", s)).collect::<Vec<_>>(),
                    })).unwrap_or_default(),
                    error: None,
                }
            }
            Err(e) => ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Trade failed: {}", e)),
            },
        }
    }
}

// ============================================================================
// CancelOrderTool
// ============================================================================

pub struct CancelOrderTool {
    ctx: Arc<HyperliquidContext>,
}

impl CancelOrderTool {
    pub fn new(ctx: Arc<HyperliquidContext>) -> Self { Self { ctx } }
}

#[async_trait]
impl Tool for CancelOrderTool {
    fn name(&self) -> &str { "cancel_order" }

    fn description(&self) -> &str {
        "Cancel an order by oid (order ID). Requires coin."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::cancel_order().to_function_schema()
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Invalid JSON: {}", e)),
            },
        };

        let coin = args["coin"].as_str().unwrap_or("BTC").to_uppercase();
        let oid: u64 = match args.get("oid").and_then(|v| v.as_u64()) {
            Some(id) if id > 0 => id,
            _ => return ToolOutput {
                success: false, content: String::new(),
                error: Some("oid (order ID) is required".into()),
            },
        };

        // Find asset index
        let markets = match self.ctx.client.perps().await {
            Ok(m) => m,
            Err(e) => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Failed to fetch markets: {}", e)),
            },
        };

        let market = match markets.iter().find(|m| m.name.eq_ignore_ascii_case(&coin)) {
            Some(m) => m,
            None => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Coin '{}' not found", coin)),
            },
        };

        let nonce = chrono::Utc::now().timestamp_millis() as u64;
        let batch = BatchCancel {
            cancels: vec![Cancel { asset: market.index, oid }],
        };

        match self.ctx.client.cancel(
            self.ctx.signer.as_ref(), batch, nonce, None, None,
        ).await {
            Ok(statuses) => {
                tracing::info!("CANCEL: oid {} on {}", oid, coin);
                ToolOutput {
                    success: true,
                    content: serde_json::to_string_pretty(&serde_json::json!({
                        "cancelled": true, "coin": coin, "oid": oid,
                        "nonce": nonce,
                        "statuses": statuses.iter().map(|s| format!("{:?}", s)).collect::<Vec<_>>(),
                    })).unwrap_or_default(),
                    error: None,
                }
            }
            Err(e) => ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Cancel failed: {}", e)),
            },
        }
    }
}

// ============================================================================
// GetPositionsTool
// ============================================================================

pub struct GetPositionsTool {
    ctx: Arc<HyperliquidContext>,
}

impl GetPositionsTool {
    pub fn new(ctx: Arc<HyperliquidContext>) -> Self { Self { ctx } }
}

#[async_trait]
impl Tool for GetPositionsTool {
    fn name(&self) -> &str { "get_positions" }

    fn description(&self) -> &str {
        "Get open positions: coin, size, entry price, unrealized PnL, \
         leverage, liquidation price, margin used, ROE."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::get_positions().to_function_schema()
    }

    async fn execute(&self, _arguments: &str) -> ToolOutput {
        match self.ctx.client.clearinghouse_state(
            self.ctx.address, None::<String>,
        ).await {
            Ok(state) => {
                let positions: Vec<serde_json::Value> = state.asset_positions.iter()
                    .filter(|p| p.position.szi != Decimal::ZERO)
                    .map(|p| serde_json::json!({
                        "coin": p.position.coin,
                        "size": p.position.szi.to_string(),
                        "entry_price": p.position.entry_px
                            .map(|px| px.to_string()).unwrap_or_default(),
                        "position_value": p.position.position_value.to_string(),
                        "unrealized_pnl": p.position.unrealized_pnl.to_string(),
                        "return_on_equity": p.position.return_on_equity.to_string(),
                        "leverage": p.position.leverage.value.to_string(),
                        "liquidation_price": p.position.liquidation_px
                            .map(|lp| lp.to_string()).unwrap_or_default(),
                        "margin_used": p.position.margin_used.to_string(),
                    }))
                    .collect();

                ToolOutput {
                    success: true,
                    content: serde_json::to_string_pretty(&serde_json::json!({
                        "equity": state.margin_summary.account_value.to_string(),
                        "margin_used": state.margin_summary.total_margin_used.to_string(),
                        "available_margin": state.margin_summary.available_margin().to_string(),
                        "total_ntl": state.margin_summary.total_ntl_pos.to_string(),
                        "withdrawable": state.withdrawable.to_string(),
                        "positions": positions,
                        "count": positions.len(),
                    })).unwrap_or_default(),
                    error: None,
                }
            }
            Err(e) => ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Failed: {}", e)),
            },
        }
    }
}

// ============================================================================
// GetMarketDataTool
// ============================================================================

pub struct GetMarketDataTool {
    ctx: Arc<HyperliquidContext>,
}

impl GetMarketDataTool {
    pub fn new(ctx: Arc<HyperliquidContext>) -> Self { Self { ctx } }
}

#[async_trait]
impl Tool for GetMarketDataTool {
    fn name(&self) -> &str { "get_market_data" }

    fn description(&self) -> &str {
        "Get market data: mid price, max leverage, config, recent funding."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::get_market_data().to_function_schema()
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Invalid JSON: {}", e)),
            },
        };
        let coin = args["coin"].as_str().unwrap_or("BTC").to_uppercase();

        let mids = match self.ctx.client.all_mids(None).await {
            Ok(m) => m,
            Err(e) => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Prices: {}", e)),
            },
        };

        let mid = mids.get(&coin).map(|d| d.to_string()).unwrap_or_else(|| "N/A".into());

        let markets = match self.ctx.client.perps().await {
            Ok(m) => m,
            Err(e) => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Markets: {}", e)),
            },
        };

        match markets.iter().find(|m| m.name.eq_ignore_ascii_case(&coin)) {
            Some(m) => {
                let now = chrono::Utc::now().timestamp_millis() as u64;
                let funding = self.ctx.client.funding_history(
                    &m.name, now - 3_600_000, Some(now),
                ).await.ok();

                ToolOutput {
                    success: true,
                    content: serde_json::to_string_pretty(&serde_json::json!({
                        "coin": m.name,
                        "mid_price": mid,
                        "max_leverage": m.max_leverage,
                        "sz_decimals": m.sz_decimals,
                        "index": m.index,
                        "recent_funding": funding.map(|f| f.iter().map(|fr|
                            serde_json::json!({
                                "time": fr.time,
                                "funding_rate": fr.funding_rate.to_string(),
                                "premium": fr.premium.to_string(),
                            })
                        ).collect::<Vec<_>>()),
                    })).unwrap_or_default(),
                    error: None,
                }
            }
            None => ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Coin '{}' not found", coin)),
            },
        }
    }
}

// ============================================================================
// GetAccountInfoTool
// ============================================================================

pub struct GetAccountInfoTool {
    ctx: Arc<HyperliquidContext>,
}

impl GetAccountInfoTool {
    pub fn new(ctx: Arc<HyperliquidContext>) -> Self { Self { ctx } }
}

#[async_trait]
impl Tool for GetAccountInfoTool {
    fn name(&self) -> &str { "get_account_info" }

    fn description(&self) -> &str {
        "Account summary: address, equity, margin, withdrawable, positions."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        TradingSkillRegistry::get_account_info().to_function_schema()
    }

    async fn execute(&self, _arguments: &str) -> ToolOutput {
        match self.ctx.client.clearinghouse_state(
            self.ctx.address, None::<String>,
        ).await {
            Ok(state) => {
                let ms = &state.margin_summary;
                ToolOutput {
                    success: true,
                    content: serde_json::to_string_pretty(&serde_json::json!({
                        "address": self.ctx.address.to_string(),
                        "chain": if self.ctx.is_testnet { "testnet" } else { "mainnet" },
                        "equity": ms.account_value.to_string(),
                        "margin_used": ms.total_margin_used.to_string(),
                        "available_margin": ms.available_margin().to_string(),
                        "utilization_pct": ms.margin_utilization().to_string(),
                        "total_ntl": ms.total_ntl_pos.to_string(),
                        "withdrawable": state.withdrawable.to_string(),
                        "cross_margin_used": state.cross_maintenance_margin_used.to_string(),
                        "positions": state.asset_positions.iter()
                            .filter(|p| p.position.szi != Decimal::ZERO).count(),
                    })).unwrap_or_default(),
                    error: None,
                }
            }
            Err(e) => ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Failed: {}", e)),
            },
        }
    }
}

// ============================================================================
// Registration
// ============================================================================

pub fn register_trading_tools(
    registry: &mut arli_core::tools::ToolRegistry,
    ctx: Arc<HyperliquidContext>,
) {
    registry.register(Box::new(ExecuteTradeTool::new(ctx.clone())));
    registry.register(Box::new(CancelOrderTool::new(ctx.clone())));
    registry.register(Box::new(GetPositionsTool::new(ctx.clone())));
    registry.register(Box::new(GetMarketDataTool::new(ctx.clone())));
    registry.register(Box::new(GetAccountInfoTool::new(ctx)));
    tracing::info!("Registered 5 Hyperliquid trading tools (live via hypersdk v0.2.12)");
}
