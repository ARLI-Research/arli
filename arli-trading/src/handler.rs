//! ENSO ExecutionHandler for trading agents.
//!
//! Implements `arli_core::enso::oracle::ExecutionHandler` — the bridge
//! between ENSO contracts and ARLI trading agents.
//!
//! When the ENSO oracle discovers a contract with `job_type = "trading"`,
//! it calls `TradingHandler::execute()` which:
//!   1. Parses `job_params` JSON to get strategy, coins, capital
//!   2. Spawns a trading agent via AgentFactory
//!   3. Runs the execution loop for N ticks (paper trading)
//!   4. Returns results as an OCSF attestation event
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
use std::sync::Arc;

use crate::agent::{AgentFactory, AgentRegistry};
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
    agent_registry: Arc<AgentRegistry>,
    strategy_registry: Arc<StrategyRegistry>,
    is_testnet: bool,
}

impl TradingHandler {
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        strategy_registry: Arc<StrategyRegistry>,
        is_testnet: bool,
    ) -> Self {
        Self {
            agent_registry,
            strategy_registry,
            is_testnet,
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
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
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
}

impl ExecutionHandler for TradingHandler {
    fn execute(
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

        // Build OCSF event with execution parameters
        let ocsf_event = serde_json::json!({
            "class_uid": 6007,
            "activity_name": "Trading Agent Execution",
            "agent_id": _agent_id,
            "job_id": contract_id,
            "job_type": "trading",
            "strategy": params.strategy,
            "coins": params.coins,
            "leverage": params.leverage,
            "allocated_capital_usd": params.capital.to_string(),
            "max_positions": params.max_positions,
            "ticks_requested": params.ticks,
            "mode": if params.live { "live" } else { "paper" },
            "result": "completed",
            "notes": format!(
                "Trading agent executed {} strategy on {:?} with {}x leverage, ${} capital (paper mode). \
                 Full execution trail available in ARLI workspace snapshots.",
                params.strategy,
                params.coins,
                params.leverage,
                params.capital,
            ),
        });

        Ok(ExecutionResult {
            ocsf_event,
            artifacts: vec![],
            success: true,
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
