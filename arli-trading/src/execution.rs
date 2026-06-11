//! Execution loop — the autonomous heartbeat of a trading agent.
//!
//! Each agent runs one execution loop:
//!
//!   loop {
//!       snapshot = fetch_market_data(ctx, strategy.watchlist())
//!       state    = fetch_agent_state(ctx)
//!       signals  = strategy.evaluate(snapshot, state, context).await
//!
//!       for signal in signals {
//!           action = risk.check(signal, state)
//!           if action == Approved { execute(ctx, signal, size) }
//!       }
//!
//!       sleep(tick_interval)
//!   }
//!
//! The loop runs until: circuit breaker triggers, manual pause, or
//! capital falls below minimum.

use crate::client::HyperliquidContext;
use crate::strategy::{
    AgentState, Direction, MarketSnapshot, OrderView, PositionSize, PositionView, Signal,
    SignalAction, Strategy,
};
use chrono::Utc;
use hypersdk::hypercore::types::*;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────────
// Execution context
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for a single agent's execution loop.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Unique agent ID.
    pub agent_id: String,
    /// Allocated capital in USD. Agent cannot exceed this in total position value.
    pub allocated_capital: Decimal,
    /// Minimum equity before agent auto-pauses.
    pub min_equity: Decimal,
    /// Maximum daily drawdown as fraction of allocated capital (0.0–1.0).
    pub max_daily_drawdown: Decimal,
    /// Tick interval in seconds.
    pub tick_interval_seconds: u64,
    /// Maximum concurrent positions.
    pub max_positions: usize,
    /// Whether to execute real trades (false = dry-run).
    pub live: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            agent_id: "default".into(),
            allocated_capital: Decimal::new(1000, 0),
            min_equity: Decimal::new(100, 0),
            max_daily_drawdown: Decimal::new(2, 1), // 0.2 = 20%
            tick_interval_seconds: 60,
            max_positions: 3,
            live: false,
        }
    }
}

/// Runtime state of the execution loop, updated each tick.
#[derive(Debug, Clone, Default)]
pub struct LoopState {
    pub tick_count: u64,
    pub total_trades: u64,
    pub winning_trades: u64,
    pub total_pnl: Decimal,
    pub peak_equity: Decimal,
    pub current_drawdown: Decimal,
    pub equity_history: Vec<(u64, Decimal)>, // (tick, equity)
    pub last_error: Option<String>,
    pub paused: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Risk check result
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskDecision {
    /// Approved for execution.
    Approved,
    /// Rejected with reason.
    Rejected(String),
    /// Circuit breaker triggered — stop the loop.
    Halt(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// The execution loop
// ─────────────────────────────────────────────────────────────────────────────

/// Run the autonomous trading loop.
///
/// Returns the final `LoopState` when the loop stops (circuit breaker, manual
/// pause, or panic-level error).
pub async fn run_loop(
    ctx: Arc<HyperliquidContext>,
    strategy: Box<dyn Strategy>,
    config: AgentConfig,
    running: Arc<AtomicBool>,
) -> LoopState {
    let mut state = LoopState::default();
    let context: HashMap<String, String> = HashMap::new();

    // Use a boxed strategy so it can be passed into async closures
    let strategy: Arc<dyn Strategy> = Arc::from(strategy);

    let tick_duration = Duration::from_secs(config.tick_interval_seconds);
    let watchlist = strategy.watchlist().to_vec();

    tracing::info!(
        agent_id = %config.agent_id,
        strategy = %strategy.name(),
        capital = %config.allocated_capital,
        interval_s = %config.tick_interval_seconds,
        live = %config.live,
        "Execution loop started"
    );

    loop {
        // ── Check stop signal ───────────────────────────────────────────
        if !running.load(Ordering::Relaxed) {
            tracing::info!(agent_id = %config.agent_id, "Stop signal received, exiting");
            state.paused = true;
            break;
        }

        // ── Fetch market data ───────────────────────────────────────────
        let snapshot = match fetch_snapshot(&ctx, &watchlist).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(agent_id = %config.agent_id, error = %e, "Snapshot failed, retrying");
                state.last_error = Some(e.to_string());
                tokio::time::sleep(tick_duration).await;
                continue;
            }
        };

        // ── Fetch agent state ───────────────────────────────────────────
        let agent_state = match fetch_agent_state(&ctx).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(agent_id = %config.agent_id, error = %e, "State fetch failed");
                state.last_error = Some(e.to_string());
                tokio::time::sleep(tick_duration).await;
                continue;
            }
        };

        // ── Circuit breaker check ──────────────────────────────────────
        if let RiskDecision::Halt(reason) = check_circuit_breaker(&state, &config) {
            tracing::error!(agent_id = %config.agent_id, %reason, "Circuit breaker triggered");
            state.last_error = Some(reason);
            break;
        }

        // ── Evaluate strategy ──────────────────────────────────────────
        let signals = strategy.evaluate(&snapshot, &agent_state, &context).await;

        if !signals.is_empty() {
            tracing::info!(
                agent_id = %config.agent_id,
                tick = %state.tick_count,
                signal_count = %signals.len(),
                "Signals generated"
            );
        }

        // ── Process signals ────────────────────────────────────────────
        for signal in &signals {
            // Risk check
            match check_risk(signal, &agent_state, &state, &config) {
                RiskDecision::Approved => {}
                RiskDecision::Rejected(reason) => {
                    tracing::info!(
                        agent_id = %config.agent_id,
                        coin = %signal.coin,
                        direction = %signal.direction.as_str(),
                        %reason,
                        "Signal rejected"
                    );
                    continue;
                }
                RiskDecision::Halt(reason) => {
                    tracing::error!(agent_id = %config.agent_id, %reason, "Risk halt");
                    state.last_error = Some(reason.clone());
                    return state;
                }
            }

            // Size position
            let size = strategy.size_position(
                signal,
                agent_state.available_margin,
                50, // max leverage from Hyperliquid
            );

            // Execute
            if config.live {
                match execute_signal(&ctx, signal, &size).await {
                    Ok(()) => {
                        state.total_trades += 1;
                        tracing::info!(
                            agent_id = %config.agent_id,
                            coin = %signal.coin,
                            direction = %signal.direction.as_str(),
                            size_usd = %size.size_usd,
                            "Trade executed"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            agent_id = %config.agent_id,
                            coin = %signal.coin,
                            error = %e,
                            "Trade execution failed"
                        );
                        state.last_error = Some(e.to_string());
                    }
                }
            } else {
                tracing::info!(
                    agent_id = %config.agent_id,
                    coin = %signal.coin,
                    direction = %signal.direction.as_str(),
                    size_usd = %size.size_usd,
                    reason = %signal.reason,
                    "DRY RUN — would execute"
                );
                state.total_trades += 1;
            }
        }

        // ── Update performance ─────────────────────────────────────────
        state.tick_count += 1;
        state.equity_history.push((state.tick_count, agent_state.equity));

        if agent_state.equity > state.peak_equity {
            state.peak_equity = agent_state.equity;
        }
        if state.peak_equity > Decimal::ZERO {
            state.current_drawdown =
                (state.peak_equity - agent_state.equity) / state.peak_equity;
        }

        // ── Wait for next tick ─────────────────────────────────────────
        tokio::time::sleep(tick_duration).await;
    }

    tracing::info!(
        agent_id = %config.agent_id,
        ticks = %state.tick_count,
        trades = %state.total_trades,
        pnl = %state.total_pnl,
        "Execution loop stopped"
    );

    state
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn fetch_snapshot(
    ctx: &HyperliquidContext,
    watchlist: &[String],
) -> anyhow::Result<MarketSnapshot> {
    let markets = ctx.client.perps().await?;
    let mids = ctx.client.all_mids(None).await?;
    let now = Utc::now().timestamp_millis() as u64;

    let mut funding: HashMap<String, Vec<crate::strategy::FundingSample>> = HashMap::new();
    for coin in watchlist {
        if let Some(m) = markets.iter().find(|m| m.name.eq_ignore_ascii_case(coin)) {
            let history_result = ctx
                .client
                .funding_history(&m.name, now - 3_600_000, Some(now))
                .await;
            if let Ok(history) = history_result
            {
                funding.insert(
                    coin.clone(),
                    history
                        .iter()
                        .map(|f| crate::strategy::FundingSample {
                            time: f.time,
                            funding_rate: f.funding_rate,
                            premium: f.premium,
                        })
                        .collect(),
                );
            }
        }
    }

    Ok(MarketSnapshot {
        mids,
        markets,
        funding,
        timestamp_ms: now,
    })
}

async fn fetch_agent_state(ctx: &HyperliquidContext) -> anyhow::Result<AgentState> {
    let cs = ctx
        .client
        .clearinghouse_state(ctx.address, None::<String>)
        .await?;

    let positions: Vec<PositionView> = cs
        .asset_positions
        .iter()
        .filter(|p| p.position.szi != Decimal::ZERO)
        .map(|p| PositionView {
            coin: p.position.coin.clone(),
            size: p.position.szi,
            entry_price: p.position.entry_px.unwrap_or_default(),
            unrealized_pnl: p.position.unrealized_pnl,
            leverage: p.position.leverage.value as u32,
            liquidation_price: p.position.liquidation_px,
        })
        .collect();

    let open_orders: Vec<OrderView> = match ctx.client.open_orders(ctx.address, None).await {
        Ok(orders) => orders
            .into_iter()
            .map(|o| OrderView {
                oid: o.oid,
                coin: o.coin.clone(),
                is_buy: o.side == Side::Bid,
                size: o.orig_sz,
                limit_price: o.limit_px,
                reduce_only: o.reduce_only,
            })
            .collect(),
        Err(_) => vec![],
    };

    Ok(AgentState {
        equity: cs.margin_summary.account_value,
        margin_used: cs.margin_summary.total_margin_used,
        available_margin: cs.margin_summary.available_margin(),
        total_pnl_all_time: Decimal::ZERO, // tracked separately
        positions,
        open_orders,
        tick_count: 0,
    })
}

fn check_circuit_breaker(state: &LoopState, config: &AgentConfig) -> RiskDecision {
    if state.current_drawdown > config.max_daily_drawdown {
        return RiskDecision::Halt(format!(
            "Max drawdown exceeded: {:.2}% > {:.2}%",
            state.current_drawdown * Decimal::from(100),
            config.max_daily_drawdown * Decimal::from(100),
        ));
    }

    // Check equity from history
    if let Some((_, last_equity)) = state.equity_history.last() {
        if *last_equity < config.min_equity {
            return RiskDecision::Halt(format!(
                "Equity below minimum: {} < {}",
                last_equity, config.min_equity,
            ));
        }
    }

    RiskDecision::Approved
}

fn check_risk(
    signal: &Signal,
    agent: &AgentState,
    _loop_state: &LoopState,
    config: &AgentConfig,
) -> RiskDecision {
    // Position limit
    if agent.positions.len() >= config.max_positions
        && signal.action == SignalAction::Enter
    {
        // Allow if it's adjusting/closing an existing position
        let has_position = agent
            .positions
            .iter()
            .any(|p| p.coin.eq_ignore_ascii_case(&signal.coin));
        if !has_position {
            return RiskDecision::Rejected(format!(
                "Max positions ({}) reached",
                config.max_positions
            ));
        }
    }

    // No duplicate entries on same coin
    if signal.action == SignalAction::Enter {
        let has_position = agent
            .positions
            .iter()
            .any(|p| p.coin.eq_ignore_ascii_case(&signal.coin));
        if has_position {
            return RiskDecision::Rejected(format!(
                "Already have position in {}",
                signal.coin
            ));
        }
    }

    // Exit requires existing position
    if signal.action == SignalAction::Exit {
        let has_position = agent
            .positions
            .iter()
            .any(|p| p.coin.eq_ignore_ascii_case(&signal.coin));
        if !has_position {
            return RiskDecision::Rejected(format!(
                "No position in {} to exit",
                signal.coin
            ));
        }
    }

    RiskDecision::Approved
}

async fn execute_signal(
    ctx: &HyperliquidContext,
    signal: &Signal,
    size: &PositionSize,
) -> anyhow::Result<()> {
    if size.size_usd <= Decimal::ZERO {
        anyhow::bail!("Zero size — nothing to execute");
    }

    let markets = ctx.client.perps().await?;
    let market = markets
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(&signal.coin))
        .ok_or_else(|| anyhow::anyhow!("Coin '{}' not found", signal.coin))?;

    let nonce = Utc::now().timestamp_millis() as u64;

    match signal.action {
        SignalAction::Enter | SignalAction::Adjust => {
            let is_buy = signal.direction == Direction::Long;

            // Set leverage first
            if size.leverage != 1 {
                ctx.client
                    .update_leverage(
                        ctx.signer.as_ref(),
                        market.index,
                        true,
                        size.leverage,
                        nonce,
                        None,
                        None,
                    )
                    .await?;
            }

            let nonce2 = Utc::now().timestamp_millis() as u64;

            match size.order_type {
                crate::strategy::OrderKind::Market => {
                    let worst_price = if is_buy {
                        Decimal::MAX / Decimal::TEN
                    } else {
                        Decimal::ZERO
                    };
                    ctx.client
                        .market_open(
                            ctx.signer.as_ref(),
                            market,
                            is_buy,
                            worst_price,
                            size.size_usd,
                            nonce2,
                            None,
                            None,
                            None,
                        )
                        .await?;
                }
                crate::strategy::OrderKind::Limit => {
                    let limit_px = signal.trigger_price.unwrap_or_else(|| {
                        if is_buy {
                            Decimal::ZERO
                        } else {
                            Decimal::MAX / Decimal::TEN
                        }
                    });

                    let batch = BatchOrder {
                        orders: vec![OrderRequest {
                            asset: market.index,
                            is_buy,
                            limit_px,
                            sz: size.size_usd,
                            reduce_only: false,
                            order_type: OrderTypePlacement::Limit {
                                tif: TimeInForce::Gtc,
                            },
                            cloid: Default::default(),
                        }],
                        grouping: OrderGrouping::Na,
                        builder: None,
                    };
                    ctx.client
                        .place(ctx.signer.as_ref(), batch, nonce2, None, None)
                        .await?;
                }
            }
        }

        SignalAction::Exit => {
            // Close position: market order in opposite direction
            // Get current position size for this coin
            let cs = ctx
                .client
                .clearinghouse_state(ctx.address, None::<String>)
                .await?;

            let pos = cs
                .asset_positions
                .iter()
                .find(|p| p.position.coin.eq_ignore_ascii_case(&signal.coin))
                .ok_or_else(|| anyhow::anyhow!("No position in {}", signal.coin))?;

            let close_size = pos.position.szi.abs();
            let is_buy = pos.position.szi < Decimal::ZERO; // short → buy to close

            let worst_price = if is_buy {
                Decimal::MAX / Decimal::TEN
            } else {
                Decimal::ZERO
            };

            ctx.client
                .market_open(
                    ctx.signer.as_ref(),
                    market,
                    is_buy,
                    worst_price,
                    close_size,
                    nonce,
                    None,
                    None,
                    None,
                )
                .await?;
        }
    }

    Ok(())
}
