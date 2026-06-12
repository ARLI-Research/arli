//! ENSO ExecutionHandler for trading agents.
//!
//! Implements `arli_core::enso::oracle::ExecutionHandler` — the bridge
//! between ENSO contracts and ARLI trading agents.
//!
//! When the ENSO oracle discovers a contract with `job_type = "trading"`,
//! it calls `TradingHandler::execute()` which:
//!   1. Parses `job_params` JSON to get strategy, coins, capital
//!   2. Connects to Hyperliquid via hypersdk
//!   3. Runs the execution loop for N ticks (paper or live)
//!   4. Returns results + metrics as an OCSF attestation event
//!
//! # Safety
//!
//! The handler runs in paper/dry-run mode by default. Set `live: true`
//! in job_params to execute real orders. This should only be done after
//! the ENSO contract has been reviewed and escrow is in place.

use arli_core::enso::oracle::{ExecutionHandler, ExecutionResult};
use arli_core::enso::JobDetail;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::execution::{self, AgentConfig};
use crate::strategy::StrategyRegistry;

/// Trading-specific execution handler for ENSO contracts.
///
/// # Example job_params
///
/// ```json
/// {
///   "strategy": "bollinger_bands",
///   "coins": ["BTC", "ETH", "SOL"],
///   "timeframe": "1h",
///   "leverage": 3,
///   "allocated_capital_usd": 1000,
///   "max_positions": 3,
///   "ticks": 10
/// }
/// ```
pub struct TradingHandler {
    strategy_registry: Arc<StrategyRegistry>,
    is_testnet: bool,
    /// Hyperliquid private key (0x-prefixed). None = testnet-only.
    hl_private_key: Option<String>,
}

impl TradingHandler {
    /// Create a new TradingHandler.
    ///
    /// `strategy_registry` — available strategies (mean_reversion, trend_following, etc.)
    /// `is_testnet` — use Hyperliquid testnet (ignored if hl_private_key is None)
    /// `hl_private_key` — optional Hyperliquid private key for live trading
    pub fn new(
        strategy_registry: Arc<StrategyRegistry>,
        is_testnet: bool,
        hl_private_key: Option<String>,
    ) -> Self {
        Self {
            strategy_registry,
            is_testnet,
            hl_private_key,
        }
    }

    /// Parse job_params JSON into trading parameters.
    fn parse_params(job: &JobDetail) -> Result<TradingParams, String> {
        let params: serde_json::Value =
            serde_json::from_str(&job.job_params).map_err(|e| format!("parse job_params: {e}"))?;

        let strategy = params["strategy"]
            .as_str()
            .unwrap_or("passive")
            .to_string();

        let coins: Vec<String> = params["coins"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_else(|| vec!["BTC".into()]);

        let leverage = params["leverage"].as_u64().unwrap_or(3) as u32;
        let capital = params["allocated_capital_usd"]
            .as_f64()
            .map(|c| Decimal::from_f64_retain(c).unwrap_or(dec!(1000)))
            .unwrap_or(dec!(1000));

        let max_positions = params["max_positions"].as_u64().unwrap_or(3) as usize;
        let ticks = params["ticks"].as_u64().unwrap_or(5) as u64;

        // Whether to execute real trades (default: paper/dry-run)
        let live = params["live"].as_bool().unwrap_or(false);

        Ok(TradingParams {
            strategy,
            coins,
            leverage,
            capital,
            max_positions,
            ticks,
            live,
        })
    }

    /// Create a Hyperliquid context from the handler config + contract params.
    fn create_hl_context(&self, params: &TradingParams) -> Result<crate::client::HyperliquidContext, String> {
        if params.live {
            // Live trading requires a private key
            let key = self
                .hl_private_key
                .as_deref()
                .ok_or_else(|| "Live trading requested but no HYPERLIQUID_PRIVATE_KEY configured".to_string())?;
            std::env::set_var("HYPERLIQUID_PRIVATE_KEY", key);
            if self.is_testnet {
                std::env::set_var("HYPERLIQUID_TESTNET", "true");
            }
        } else {
            // Paper trading — use testnet with a dummy key
            // Generate a throwaway key for data-only access (no funds needed)
            std::env::set_var("HYPERLIQUID_TESTNET", "true");
            // Use a known testnet key that can read market data without trading
            if std::env::var("HYPERLIQUID_PRIVATE_KEY").is_err() {
                std::env::set_var(
                    "HYPERLIQUID_PRIVATE_KEY",
                    "0x0000000000000000000000000000000000000000000000000000000000000001",
                );
            }
        }

        crate::client::HyperliquidContext::from_env()
            .map_err(|e| format!("Failed to initialize Hyperliquid: {e}"))
    }
}

#[async_trait::async_trait]
impl ExecutionHandler for TradingHandler {
    async fn execute(
        &self,
        contract_id: &str,
        _agent_id: &str,
        job: &JobDetail,
    ) -> Result<ExecutionResult, String> {
        if job.job_type != "trading" {
            return Err(format!(
                "TradingHandler: unsupported job_type '{}', expected 'trading'",
                job.job_type
            ));
        }

        let start = std::time::Instant::now();
        let params = Self::parse_params(job)?;

        tracing::info!(
            contract = %contract_id,
            strategy = %params.strategy,
            coins = ?params.coins,
            capital = %params.capital,
            leverage = %params.leverage,
            ticks = %params.ticks,
            live = %params.live,
            "TradingHandler: executing ENSO contract"
        );

        // Build the strategy
        let strategy = self
            .strategy_registry
            .build(&params.strategy)
            .ok_or_else(|| format!("Unknown strategy: {}", params.strategy))?;

        // Initialize Hyperliquid context
        let ctx = self.create_hl_context(&params)?;

        // Agent config from contract params
        let config = AgentConfig {
            agent_id: format!("enso-{}", contract_id),
            allocated_capital: params.capital,
            min_equity: params.capital / dec!(10), // 10% of capital
            max_daily_drawdown: Decimal::new(2, 1), // 20%
            tick_interval_seconds: 60,
            max_positions: params.max_positions,
            live: params.live,
        };

        // Execution loop — limited to `ticks` iterations via stop signal
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let ctx = Arc::new(ctx);
        let ctx_clone = ctx.clone();
        let strategy_name = params.strategy.clone();
        let coins = params.coins.clone();
        let max_ticks = params.ticks;

        let tick_interval = config.tick_interval_seconds;

        // Spawn the loop in background, stop it after max_ticks
        let loop_handle = tokio::spawn(async move {
            execution::run_loop(ctx_clone, strategy, config, running_clone).await
        });

        // Wait for the loop to run enough ticks, then stop it
        tokio::time::sleep(std::time::Duration::from_secs(
            max_ticks * tick_interval,
        )).await;
        running.store(false, std::sync::atomic::Ordering::Relaxed);

        let loop_state = loop_handle
            .await
            .map_err(|e| format!("Execution loop panicked: {e}"))?;

        let elapsed_ms = start.elapsed().as_millis() as u64;

        tracing::info!(
            contract = %contract_id,
            elapsed_ms = %elapsed_ms,
            ticks = %loop_state.tick_count,
            trades = %loop_state.total_trades,
            pnl = %loop_state.total_pnl,
            "TradingHandler: execution complete"
        );

        // Check success before moving last_error into OCSF event
        let success = loop_state.last_error.is_none();
        let error_msg = loop_state.last_error.clone().unwrap_or_default();

        // Build OCSF event with execution results
        let ocsf_event = serde_json::json!({
            "class_uid": 6007,
            "activity_name": "Trading Agent Execution",
            "agent_id": _agent_id,
            "job_id": contract_id,
            "job_type": "trading",
            "strategy": strategy_name,
            "coins": coins,
            "leverage": params.leverage,
            "allocated_capital_usd": params.capital.to_string(),
            "max_positions": params.max_positions,
            "ticks_requested": params.ticks,
            "ticks_executed": loop_state.tick_count,
            "trades_executed": loop_state.total_trades,
            "winning_trades": loop_state.winning_trades,
            "total_pnl": loop_state.total_pnl.to_string(),
            "peak_equity": loop_state.peak_equity.to_string(),
            "max_drawdown_pct": (loop_state.current_drawdown * dec!(100)).to_string(),
            "mode": if params.live { "live" } else { "paper" },
            "result": if success { "completed" } else { "error" },
            "error": error_msg,
            "notes": format!(
                "Trading agent executed {} strategy on {:?} with {}x leverage, ${} capital ({} mode). \
                 {} ticks, {} trades, PnL: ${}.",
                strategy_name, coins, params.leverage, params.capital,
                if params.live { "live" } else { "paper" },
                loop_state.tick_count, loop_state.total_trades, loop_state.total_pnl,
            ),
        });

        // Build execution metrics for ENSO SLA enforcement
        let metrics = serde_json::json!({
            "execution_latency_ms": elapsed_ms,
            "strategy": strategy_name,
            "coins": coins.len(),
            "leverage": params.leverage,
            "capital_usd": params.capital.to_string(),
            "ticks_evaluated": loop_state.tick_count,
            "trades_executed": loop_state.total_trades,
            "total_pnl": loop_state.total_pnl.to_string(),
            "max_drawdown_pct": (loop_state.current_drawdown * dec!(100)).to_string(),
            "mode": if params.live { "live" } else { "paper" },
        });

        Ok(ExecutionResult {
            ocsf_event,
            artifacts: vec![],
            success,
            metrics: Some(metrics),
        })
    }
}

/// Parsed trading parameters from ENSO job_params.
struct TradingParams {
    strategy: String,
    coins: Vec<String>,
    leverage: u32,
    capital: Decimal,
    max_positions: usize,
    ticks: u64,
    live: bool,
}
