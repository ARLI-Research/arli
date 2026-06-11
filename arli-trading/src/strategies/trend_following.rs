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

pub struct TrendFollowingStrategy {
    pub fast_period: usize,
    pub slow_period: usize,
    pub min_crossover_strength: Decimal,
    pub max_leverage: u32,
    pub watchlist: Vec<String>,
    pub tick_interval_s: u64,
    price_history_len: usize,
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
            price_history_len: 30,
        }
    }
}

impl TrendFollowingStrategy {
    fn parse_history(context: &HashMap<String, String>, coin: &str) -> Vec<Decimal> {
        context
            .get(&format!("tf:{}:prices", coin))
            .map(|s| s.split(',').filter_map(|v| v.parse::<Decimal>().ok()).collect())
            .unwrap_or_default()
    }

    fn store_history(context: &mut HashMap<String, String>, coin: &str, prices: &[Decimal]) {
        let s: String = prices.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(",");
        context.insert(format!("tf:{}:prices", coin), s);
    }

    fn parse_ema(context: &HashMap<String, String>, coin: &str, period: usize) -> Option<Decimal> {
        context.get(&format!("tf:{}:ema{}", coin, period)).and_then(|s| s.parse::<Decimal>().ok())
    }

    fn store_ema(context: &mut HashMap<String, String>, coin: &str, period: usize, ema: Decimal) {
        context.insert(format!("tf:{}:ema{}", coin, period), ema.to_string());
    }

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
        context: &HashMap<String, String>,
    ) -> Vec<Signal> {
        let mut signals = Vec::new();
        let mut new_ctx = context.clone();

        for coin in &self.watchlist {
            let price = match snapshot.mids.get(coin.as_str()) {
                Some(p) => *p,
                None => continue,
            };

            let mut history = Self::parse_history(&new_ctx, coin);
            history.push(price);
            if history.len() > self.price_history_len {
                history.remove(0);
            }
            Self::store_history(&mut new_ctx, coin, &history);

            let prev_fast = Self::parse_ema(&new_ctx, coin, self.fast_period);
            let prev_slow = Self::parse_ema(&new_ctx, coin, self.slow_period);
            let fast = Self::compute_ema(price, prev_fast, self.fast_period);
            let slow = Self::compute_ema(price, prev_slow, self.slow_period);
            Self::store_ema(&mut new_ctx, coin, self.fast_period, fast);
            Self::store_ema(&mut new_ctx, coin, self.slow_period, slow);

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
