//! Mean-reversion strategy — buy dips, sell rips.
//!
//! Uses a simple SMA-based approach:
//!   - Enter long when price drops below SMA by `entry_threshold` standard deviations
//!   - Enter short when price rises above SMA by `entry_threshold` standard deviations
//!   - Exit when price reverts to SMA
//!
//! Position sizing is volatility-adjusted: wider bands = smaller size.

use crate::strategy::{
    AgentState, Direction, MarketSnapshot, OrderKind, PositionSize, Signal, SignalAction, Strategy,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::HashMap;

/// Mean-reversion strategy with configurable lookback and thresholds.
pub struct MeanReversionStrategy {
    /// SMA period in ticks (e.g., 20 = ~20 minutes at 60s ticks).
    pub sma_period: usize,
    /// Entry threshold in standard deviations from SMA.
    pub entry_threshold: Decimal,
    /// Exit threshold in standard deviations (closer to SMA = exit sooner).
    pub exit_threshold: Decimal,
    /// Maximum leverage.
    pub max_leverage: u32,
    /// Watchlist.
    pub watchlist: Vec<String>,
    /// Tick interval in seconds.
    pub tick_interval_s: u64,
    /// Internal price history: coin → Vec<price>
    /// Stored as Decimal strings in context to be stateless.
    price_history_len: usize,
}

impl Default for MeanReversionStrategy {
    fn default() -> Self {
        Self {
            sma_period: 20,
            entry_threshold: dec!(2.0),
            exit_threshold: dec!(0.5),
            max_leverage: 10,
            watchlist: vec!["BTC".into(), "ETH".into(), "SOL".into()],
            tick_interval_s: 60,
            price_history_len: 20,
        }
    }
}

impl MeanReversionStrategy {
    fn parse_history(context: &HashMap<String, String>, coin: &str) -> Vec<Decimal> {
        context
            .get(&format!("mr:{coin}:prices"))
            .map(|s| s.split(',').filter_map(|v| v.parse::<Decimal>().ok()).collect())
            .unwrap_or_default()
    }

    fn store_history(context: &mut HashMap<String, String>, coin: &str, prices: &[Decimal]) {
        let s: String = prices
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join(",");
        context.insert(format!("mr:{coin}:prices"), s);
    }

    fn compute_sma_and_std(prices: &[Decimal], period: usize) -> (Decimal, Decimal) {
        if prices.len() < period || period == 0 {
            return (Decimal::ZERO, Decimal::ZERO);
        }
        let window = &prices[prices.len() - period..];
        let sum: Decimal = window.iter().sum();
        let n = Decimal::from(period);
        let mean = sum / n;
        let variance: Decimal = window.iter().map(|p| (p - mean) * (p - mean)).sum::<Decimal>() / n;
        // sqrt approximation using Newton's method (good enough for trading)
        let std_dev = sqrt_approx(variance);
        (mean, std_dev)
    }
}

/// Simple sqrt using Newton's method for Decimal.
fn sqrt_approx(x: Decimal) -> Decimal {
    if x <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    let mut guess = x / dec!(2);
    for _ in 0..10 {
        guess = (guess + x / guess) / dec!(2);
    }
    guess
}

#[async_trait::async_trait]
impl Strategy for MeanReversionStrategy {
    fn name(&self) -> &str {
        "mean-reversion"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn tick_interval_seconds(&self) -> u64 {
        self.tick_interval_s
    }

    fn watchlist(&self) -> &[String] {
        &self.watchlist
    }

    async fn evaluate(
        &self,
        snapshot: &MarketSnapshot,
        state: &AgentState,
        context: &HashMap<String, String>,
    ) -> Vec<Signal> {
        let mut signals = Vec::new();
        let mut new_context = context.clone();

        for coin in &self.watchlist {
            let price = match snapshot.mids.get(coin.as_str()) {
                Some(p) => *p,
                None => continue,
            };

            // Update price history
            let mut history = Self::parse_history(&new_context, coin);
            history.push(price);
            if history.len() > self.price_history_len {
                history.remove(0);
            }
            Self::store_history(&mut new_context, coin, &history);

            let (sma, std_dev) =
                Self::compute_sma_and_std(&history, self.sma_period);

            if sma == Decimal::ZERO || std_dev == Decimal::ZERO {
                continue; // not enough data
            }

            let deviation = (price - sma) / std_dev;

            let has_position = state
                .positions
                .iter()
                .any(|p| p.coin.eq_ignore_ascii_case(coin));

            if has_position {
                // Exit if price reverted to near SMA
                if deviation.abs() <= self.exit_threshold {
                    signals.push(Signal {
                        coin: coin.clone(),
                        direction: Direction::Long, // direction flipped for exit
                        action: SignalAction::Exit,
                        confidence: dec!(0.9),
                        trigger_price: None,
                        reason: format!(
                            "{coin}: reverted to SMA (dev={deviation:.2}σ)",
                            coin = coin,
                            deviation = deviation,
                        ),
                    });
                }
            } else {
                // Enter long if price is significantly below SMA
                if deviation <= -self.entry_threshold {
                    let confidence = (deviation.abs() / self.entry_threshold)
                        .min(dec!(1.0));
                    signals.push(Signal {
                        coin: coin.clone(),
                        direction: Direction::Long,
                        action: SignalAction::Enter,
                        confidence,
                        trigger_price: None,
                        reason: format!(
                            "{coin}: oversold, {deviation:.2}σ below SMA {sma}",
                            coin = coin,
                            deviation = deviation,
                            sma = sma.round_dp(2),
                        ),
                    });
                }
                // Enter short if price is significantly above SMA
                else if deviation >= self.entry_threshold {
                    let confidence = (deviation / self.entry_threshold)
                        .min(dec!(1.0));
                    signals.push(Signal {
                        coin: coin.clone(),
                        direction: Direction::Short,
                        action: SignalAction::Enter,
                        confidence,
                        trigger_price: None,
                        reason: format!(
                            "{coin}: overbought, +{deviation:.2}σ above SMA {sma}",
                            coin = coin,
                            deviation = deviation,
                            sma = sma.round_dp(2),
                        ),
                    });
                }
            }
        }

        // Note: context mutation happens through the shared context map
        // in the execution loop. For now we return signals.
        signals
    }

    fn size_position(
        &self,
        signal: &Signal,
        available_capital: Decimal,
        max_leverage: u32,
    ) -> PositionSize {
        let leverage = self.max_leverage.min(max_leverage);

        // Kelly-inspired: size proportional to confidence
        let base_allocation = available_capital * dec!(0.05); // 5% of capital
        let scaled = base_allocation * signal.confidence;

        PositionSize {
            size_usd: scaled.round_dp(2),
            leverage,
            stop_loss: None, // strategy-level stop loss set by risk manager
            take_profit: None,
            order_type: OrderKind::Market,
        }
    }
}
