//! Strategy trait — the core abstraction for pluggable trading strategies.
//!
//! Each ARLI agent runs one strategy. The strategy:
//!   1. Evaluates market data + current state → produces signals
//!   2. Sizes positions based on capital, risk, and conviction
//!
//! Strategies are pure decision functions. Execution, risk checks,
//! and capital isolation happen in the execution loop.

use hypersdk::hypercore::{self};
use rust_decimal::Decimal;
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────────
// Market snapshot — the data a strategy sees on each tick
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of market data the strategy evaluates each tick.
#[derive(Debug, Clone)]
pub struct MarketSnapshot {
    /// Mid prices: coin → price
    pub mids: HashMap<String, Decimal>,
    /// Per-coin market metadata
    pub markets: Vec<hypercore::PerpMarket>,
    /// Recent funding rates: coin → (time, funding_rate, premium)
    pub funding: HashMap<String, Vec<FundingSample>>,
    /// Timestamp of this snapshot (ms)
    pub timestamp_ms: u64,
}

/// A single funding rate sample.
#[derive(Debug, Clone)]
pub struct FundingSample {
    pub time: u64,
    pub funding_rate: Decimal,
    pub premium: Decimal,
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent state — current positions, capital, P&L
// ─────────────────────────────────────────────────────────────────────────────

/// Current state of the agent's account.
#[derive(Debug, Clone)]
pub struct AgentState {
    pub equity: Decimal,
    pub margin_used: Decimal,
    pub available_margin: Decimal,
    pub total_pnl_all_time: Decimal,
    pub positions: Vec<PositionView>,
    pub open_orders: Vec<OrderView>,
    pub tick_count: u64,
}

/// Simplified view of an open position.
#[derive(Debug, Clone)]
pub struct PositionView {
    pub coin: String,
    pub size: Decimal,      // signed: positive = long, negative = short
    pub entry_price: Decimal,
    pub unrealized_pnl: Decimal,
    pub leverage: u32,
    pub liquidation_price: Option<Decimal>,
}

/// Simplified view of an open order.
#[derive(Debug, Clone)]
pub struct OrderView {
    pub oid: u64,
    pub coin: String,
    pub is_buy: bool,
    pub size: Decimal,
    pub limit_price: Decimal,
    pub reduce_only: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Signals
// ─────────────────────────────────────────────────────────────────────────────

/// Direction of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Long,
    Short,
}

impl Direction {
    pub fn as_str(&self) -> &str {
        match self {
            Direction::Long => "long",
            Direction::Short => "short",
        }
    }
}

/// What the strategy wants to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalAction {
    /// Open a new position.
    Enter,
    /// Close an existing position entirely.
    Exit,
    /// Scale in/out — adjust existing position size.
    Adjust,
}

/// A trading signal produced by the strategy.
#[derive(Debug, Clone)]
pub struct Signal {
    pub coin: String,
    pub direction: Direction,
    pub action: SignalAction,
    /// Confidence 0.0–1.0. Used by risk manager to scale position size.
    pub confidence: Decimal,
    /// Optional trigger price for conditional orders.
    pub trigger_price: Option<Decimal>,
    /// Human-readable reason (logged, not used in execution).
    pub reason: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Position sizing
// ─────────────────────────────────────────────────────────────────────────────

/// How much capital to allocate to a signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionSize {
    /// Notional size in USD.
    pub size_usd: Decimal,
    /// Leverage (1–50).
    pub leverage: u32,
    /// Stop-loss price. If None, no stop-loss.
    pub stop_loss: Option<Decimal>,
    /// Take-profit price. If None, no take-profit.
    pub take_profit: Option<Decimal>,
    /// Order type.
    pub order_type: OrderKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderKind {
    Market,
    Limit,
}

// ─────────────────────────────────────────────────────────────────────────────
// The Strategy trait
// ─────────────────────────────────────────────────────────────────────────────

/// A pluggable trading strategy.
///
/// Implement this trait to define a strategy. The execution loop calls:
///   1. `evaluate()` — produce signals from market data + current state
///   2. `size_position()` — determine position sizing for a signal
///
/// Strategies are stateless decision functions. Persistent state (performance
/// history, pattern memory) lives in the ARLI shared_memory / workspace_snapshot
/// and is passed via `AgentState` + the `context` map.
#[async_trait::async_trait]
pub trait Strategy: Send + Sync {
    /// Unique strategy name (e.g., "trend-follow", "mean-reversion").
    fn name(&self) -> &str;

    /// Version string for tracking strategy changes.
    fn version(&self) -> &str;

    /// Minimum tick interval this strategy wants (in seconds).
    fn tick_interval_seconds(&self) -> u64;

    /// Which coins this strategy watches.
    fn watchlist(&self) -> &[String];

    /// Evaluate market data and current state, produce trading signals.
    ///
    /// Called each tick. The strategy receives:
    ///   - `snapshot` — current market data (prices, funding, metadata)
    ///   - `state` — agent's current positions, equity, P&L
    ///   - `context` — arbitrary key-value data for strategy-specific state
    ///     (e.g., indicator values from previous ticks)
    ///
    /// Returns a list of signals, ordered by priority. Empty vec = no action.
    async fn evaluate(
        &self,
        snapshot: &MarketSnapshot,
        state: &AgentState,
        context: &HashMap<String, String>,
    ) -> Vec<Signal>;

    /// Determine position size for a signal.
    ///
    /// Called for each signal returned by `evaluate()` that passes risk checks.
    /// The strategy decides how much of available capital to allocate.
    fn size_position(
        &self,
        signal: &Signal,
        available_capital: Decimal,
        max_leverage: u32,
    ) -> PositionSize;
}

// ─────────────────────────────────────────────────────────────────────────────
// Built-in default: Passive observer (no trades)
// ─────────────────────────────────────────────────────────────────────────────

/// A no-op strategy that watches markets but never trades.
/// Useful as a safe default for new agents.
pub struct PassiveStrategy {
    pub watchlist: Vec<String>,
}

impl PassiveStrategy {
    pub fn new(watchlist: Vec<String>) -> Self {
        Self { watchlist }
    }
}

#[async_trait::async_trait]
impl Strategy for PassiveStrategy {
    fn name(&self) -> &str {
        "passive"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn tick_interval_seconds(&self) -> u64 {
        60
    }

    fn watchlist(&self) -> &[String] {
        &self.watchlist
    }

    async fn evaluate(
        &self,
        _snapshot: &MarketSnapshot,
        _state: &AgentState,
        _context: &HashMap<String, String>,
    ) -> Vec<Signal> {
        vec![] // never trades
    }

    fn size_position(
        &self,
        _signal: &Signal,
        _available_capital: Decimal,
        _max_leverage: u32,
    ) -> PositionSize {
        PositionSize {
            size_usd: Decimal::ZERO,
            leverage: 1,
            stop_loss: None,
            take_profit: None,
            order_type: OrderKind::Market,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Strategy registry
// ─────────────────────────────────────────────────────────────────────────────

/// Registry of available strategies. Used by the agent factory to
/// instantiate a strategy by name.
pub struct StrategyRegistry {
    strategies: HashMap<String, Box<dyn Fn() -> Box<dyn Strategy> + Send + Sync>>,
}

impl StrategyRegistry {
    pub fn new() -> Self {
        Self {
            strategies: HashMap::new(),
        }
    }

    pub fn register<F>(&mut self, factory: F)
    where
        F: Fn() -> Box<dyn Strategy> + Send + Sync + 'static,
    {
        let instance = factory();
        let name = instance.name().to_string();
        self.strategies.insert(name, Box::new(factory));
    }

    pub fn build(&self, name: &str) -> Option<Box<dyn Strategy>> {
        self.strategies.get(name).map(|f| f())
    }

    pub fn names(&self) -> Vec<&str> {
        self.strategies.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for StrategyRegistry {
    fn default() -> Self {
        let mut r = Self::new();
        r.register(|| Box::new(PassiveStrategy::new(vec!["BTC".into(), "ETH".into()])));
        r
    }
}
