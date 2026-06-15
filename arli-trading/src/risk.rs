//! Risk management — per-agent capital controls, stop-loss, circuit breakers.
//!
//! Each agent has a RiskManager that enforces:
//!   - Max position size relative to allocated capital
//!   - Max daily drawdown (circuit breaker)
//!   - Max concurrent positions
//!   - Max leverage per coin
//!   - Stop-loss / take-profit on open positions

use crate::strategy::{AgentState, Direction, PositionSize, PositionView, Signal, SignalAction};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

// ─────────────────────────────────────────────────────────────────────────────
// Risk parameters
// ─────────────────────────────────────────────────────────────────────────────

/// Risk configuration for an agent.
#[derive(Debug, Clone)]
pub struct RiskParams {
    /// Allocated capital — total USD value the agent can deploy.
    pub allocated_capital: Decimal,
    /// Maximum fraction of allocated capital per position (e.g., 0.2 = 20%).
    pub max_capital_per_position: Decimal,
    /// Maximum daily drawdown as fraction of peak equity (e.g., 0.2 = 20%).
    pub max_daily_drawdown: Decimal,
    /// Maximum concurrent open positions.
    pub max_positions: usize,
    /// Default leverage if strategy doesn't specify.
    pub default_leverage: u32,
    /// Maximum allowed leverage (exchange cap).
    pub max_leverage: u32,
    /// Default stop-loss as fraction of entry price (e.g., 0.05 = 5%).
    pub default_stop_loss_pct: Decimal,
    /// Default take-profit as fraction of entry price (e.g., 0.10 = 10%).
    pub default_take_profit_pct: Decimal,
    /// Minimum equity before agent pauses.
    pub min_equity: Decimal,
}

impl Default for RiskParams {
    fn default() -> Self {
        Self {
            allocated_capital: dec!(1000),
            max_capital_per_position: dec!(0.2),
            max_daily_drawdown: dec!(0.2),
            max_positions: 3,
            default_leverage: 3,
            max_leverage: 50,
            default_stop_loss_pct: dec!(0.05),
            default_take_profit_pct: dec!(0.10),
            min_equity: dec!(100),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Risk decision
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskDecision {
    Approved(PositionSize),
    Rejected(String),
    Halt(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// Risk manager
// ─────────────────────────────────────────────────────────────────────────────

/// Evaluates each signal against risk parameters and current state.
pub struct RiskManager {
    params: RiskParams,
}

impl RiskManager {
    pub fn new(params: RiskParams) -> Self {
        Self { params }
    }

    /// Check if a signal should be executed, and produce a risk-adjusted
    /// position size.
    pub fn evaluate(
        &self,
        signal: &Signal,
        state: &AgentState,
        peak_equity: Decimal,
    ) -> RiskDecision {
        // ── Circuit breaker: drawdown check ─────────────────────────
        let drawdown = if peak_equity > Decimal::ZERO {
            (peak_equity - state.equity) / peak_equity
        } else {
            Decimal::ZERO
        };

        if drawdown > self.params.max_daily_drawdown {
            return RiskDecision::Halt(format!(
                "Drawdown {:.2}% exceeds max {:.2}%",
                drawdown * dec!(100),
                self.params.max_daily_drawdown * dec!(100),
            ));
        }

        // ── Minimum equity check ────────────────────────────────────
        if state.equity < self.params.min_equity {
            return RiskDecision::Halt(format!(
                "Equity {:.2} below minimum {:.2}",
                state.equity, self.params.min_equity,
            ));
        }

        match signal.action {
            SignalAction::Enter => self.evaluate_entry(signal, state),
            SignalAction::Exit => RiskDecision::Approved(PositionSize {
                size_usd: Decimal::ZERO, // exit means close all, size determined at execution
                leverage: 1,
                stop_loss: None,
                take_profit: None,
                order_type: crate::strategy::OrderKind::Market,
            }),
            SignalAction::Adjust => self.evaluate_entry(signal, state),
        }
    }

    fn evaluate_entry(&self, signal: &Signal, state: &AgentState) -> RiskDecision {
        // ── Max positions ───────────────────────────────────────────
        if state.positions.len() >= self.params.max_positions {
            let has_position = state
                .positions
                .iter()
                .any(|p| p.coin.eq_ignore_ascii_case(&signal.coin));
            if !has_position {
                return RiskDecision::Rejected(format!(
                    "Max positions ({}) reached",
                    self.params.max_positions,
                ));
            }
        }

        // ── Duplicate position ──────────────────────────────────────
        if signal.action == SignalAction::Enter {
            let has_position = state
                .positions
                .iter()
                .any(|p| p.coin.eq_ignore_ascii_case(&signal.coin));
            if has_position {
                return RiskDecision::Rejected(format!(
                    "Already holding {}",
                    signal.coin,
                ));
            }
        }

        // ── Position size cap ───────────────────────────────────────
        let max_size = self.params.allocated_capital
            * self.params.max_capital_per_position;

        // Calculate position size based on available margin and confidence
        let available = state.available_margin.min(self.params.allocated_capital);
        let base_size = available * dec!(0.05); // 5% of available per signal
        let confidence_scale = signal.confidence.max(dec!(0.1)).min(Decimal::ONE);
        let size = (base_size * confidence_scale).min(max_size);

        if size < dec!(10) {
            return RiskDecision::Rejected(format!(
                "Position too small: ${:.2} (min $10)",
                size,
            ));
        }

        // ── Leverage cap ────────────────────────────────────────────
        let leverage = self.params.default_leverage.min(self.params.max_leverage);

        // ── Stop-loss / take-profit ─────────────────────────────────
        let stop_loss = match signal.trigger_price {
            Some(tp) => Some(tp), // strategy-set stop
            None => {
                // Default stop at -5%
                let mid = state.positions.iter()
                    .find(|p| p.coin.eq_ignore_ascii_case(&signal.coin))
                    .map(|p| p.entry_price);
                mid.map(|entry| match signal.direction {
                    Direction::Long => entry * (Decimal::ONE - self.params.default_stop_loss_pct),
                    Direction::Short => entry * (Decimal::ONE + self.params.default_stop_loss_pct),
                })
            }
        };

        let take_profit = state.positions.iter()
            .find(|p| p.coin.eq_ignore_ascii_case(&signal.coin))
            .map(|p| p.entry_price)
            .map(|entry| match signal.direction {
                Direction::Long => entry * (Decimal::ONE + self.params.default_take_profit_pct),
                Direction::Short => entry * (Decimal::ONE - self.params.default_take_profit_pct),
            });

        RiskDecision::Approved(PositionSize {
            size_usd: size.round_dp(2),
            leverage,
            stop_loss,
            take_profit,
            order_type: crate::strategy::OrderKind::Market,
        })
    }

    /// Check if stop-loss or take-profit triggered for an existing position.
    pub fn check_position_exit(
        &self,
        position: &PositionView,
        current_price: Decimal,
    ) -> Option<SignalAction> {
        // Stop-loss
        if let Some(sl) = position.stop_loss {
            if sl > Decimal::ZERO && current_price <= sl {
                return Some(SignalAction::Exit);
            }
        }
        // Take-profit
        if let Some(tp) = position.take_profit {
            if tp > Decimal::ZERO && current_price >= tp {
                return Some(SignalAction::Exit);
            }
        }
        None
    }
}
