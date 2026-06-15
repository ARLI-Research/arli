//! Trend-following strategy — ride momentum using EMA crossover.
//!
//! Enter long when fast EMA crosses above slow EMA (bullish).
//! Enter short when fast EMA crosses below slow EMA (bearish).
//! Exit when crossover reverses. Funding rate adjusts conviction.

use crate::strategy::{
    AgentState, Direction, MarketSnapshot, OrderKind, PositionSize, Signal, SignalAction, Strategy,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct TrendFollowingStrategy {
    pub fast_period: usize,
    pub slow_period: usize,
    pub min_crossover_strength: Decimal,
    pub max_leverage: u32,
    pub watchlist: Vec<String>,
    pub tick_interval_s: u64,
    /// Internal state: coin → (fast_ema, slow_ema, prices)
    state: Mutex<HashMap<String, (Decimal, Decimal, Vec<Decimal>)>>,
}

impl Default for TrendFollowingStrategy {
    fn default() -> Self {
        Self {
            fast_period: 9,
            slow_period: 21,
            min_crossover_strength: dec!(0.001),
            max_leverage: 5,
            watchlist: vec!["BTC".into(), "ETH".into()],
            tick_interval_s: 120,
            state: Mutex::new(HashMap::new()),
        }
    }
}

impl TrendFollowingStrategy {
    fn compute_ema(price: Decimal, prev_ema: Option<Decimal>, period: usize) -> Decimal {
        let k = Decimal::from(2) / Decimal::from(period + 1);
        match prev_ema {
            Some(prev) => price * k + prev * (Decimal::ONE - k),
            None => price,
        }
    }

    fn funding_penalty(funding: &[crate::strategy::FundingSample], direction: Direction) -> Decimal {
        if funding.is_empty() {
            return Decimal::ONE;
        }
        let n = Decimal::from(funding.len());
        let avg: Decimal = funding.iter().map(|f| f.funding_rate).sum::<Decimal>() / n;
        match direction {
            Direction::Long => {
                if avg > dec!(0.001) {
                    (Decimal::ONE - avg * dec!(100)).max(dec!(0.5))
                } else {
                    Decimal::ONE
                }
            }
            Direction::Short => {
                if avg < dec!(-0.001) {
                    (Decimal::ONE + avg * dec!(100)).max(dec!(0.5))
                } else {
                    Decimal::ONE
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Strategy for TrendFollowingStrategy {
    fn name(&self) -> &str { "trend-following" }
    fn version(&self) -> &str { "1.0.0" }
    fn tick_interval_seconds(&self) -> u64 { self.tick_interval_s }
    fn watchlist(&self) -> &[String] { &self.watchlist }

    async fn evaluate(
        &self,
        snapshot: &MarketSnapshot,
        state: &AgentState,
        _context: &HashMap<String, String>,
    ) -> Vec<Signal> {
        let mut signals = Vec::new();
        let mut state_map = self.state.lock().unwrap();

        for coin in &self.watchlist {
            let price = match snapshot.mids.get(coin.as_str()) {
                Some(p) => *p,
                None => continue,
            };

            let entry = state_map.entry(coin.clone()).or_insert((Decimal::ZERO, Decimal::ZERO, Vec::new()));
            let (prev_fast, prev_slow, ref mut prices) = &mut *entry;
            prices.push(price);
            if prices.len() > 50 { prices.remove(0); }

            let fast = Self::compute_ema(price, if *prev_fast == Decimal::ZERO { None } else { Some(*prev_fast) }, self.fast_period);
            let slow = Self::compute_ema(price, if *prev_slow == Decimal::ZERO { None } else { Some(*prev_slow) }, self.slow_period);
            *prev_fast = fast;
            *prev_slow = slow;

            if slow == Decimal::ZERO {
                continue;
            }

            let crossover = (fast - slow) / slow;
            let has_position = state.positions.iter().any(|p| p.coin.eq_ignore_ascii_case(coin));
            let funding = snapshot.funding.get(coin.as_str());

            if has_position {
                let pos = state.positions.iter().find(|p| p.coin.eq_ignore_ascii_case(coin)).unwrap();
                let is_long = pos.size > Decimal::ZERO;
                let should_exit = (is_long && crossover < dec!(-0.002))
                    || (!is_long && crossover > dec!(0.002));

                if should_exit {
                    signals.push(Signal {
                        coin: coin.clone(),
                        direction: if is_long { Direction::Long } else { Direction::Short },
                        action: SignalAction::Exit,
                        confidence: dec!(0.85),
                        trigger_price: None,
                        reason: format!("{}: EMA cross reversed fast={} slow={}", coin, fast.round_dp(2), slow.round_dp(2)),
                    });
                }
            } else if crossover > self.min_crossover_strength {
                let base = (crossover / self.min_crossover_strength * dec!(0.5)).min(dec!(1.0));
                let fc = funding.map_or(Decimal::ONE, |f| Self::funding_penalty(f, Direction::Long));
                let confidence = (base * fc).min(dec!(1.0));
                if confidence >= dec!(0.3) {
                    signals.push(Signal {
                        coin: coin.clone(),
                        direction: Direction::Long,
                        action: SignalAction::Enter,
                        confidence,
                        trigger_price: None,
                        reason: format!("{}: bullish EMA cross fast={} slow={}", coin, fast.round_dp(2), slow.round_dp(2)),
                    });
                }
            } else if crossover < -self.min_crossover_strength {
                let base = ((-crossover) / self.min_crossover_strength * dec!(0.5)).min(dec!(1.0));
                let fc = funding.map_or(Decimal::ONE, |f| Self::funding_penalty(f, Direction::Short));
                let confidence = (base * fc).min(dec!(1.0));
                if confidence >= dec!(0.3) {
                    signals.push(Signal {
                        coin: coin.clone(),
                        direction: Direction::Short,
                        action: SignalAction::Enter,
                        confidence,
                        trigger_price: None,
                        reason: format!("{}: bearish EMA cross fast={} slow={}", coin, fast.round_dp(2), slow.round_dp(2)),
                    });
                }
            }
        }

        signals
    }

    fn size_position(
        &self,
        signal: &Signal,
        available_capital: Decimal,
        max_leverage: u32,
    ) -> PositionSize {
        let leverage = self.max_leverage.min(max_leverage);
        let base = available_capital * dec!(0.08);
        let scaled = base * signal.confidence;
        PositionSize {
            size_usd: scaled.round_dp(2),
            leverage,
            stop_loss: None,
            take_profit: None,
            order_type: OrderKind::Market,
        }
    }
}
