//! Volume-Weighted Average Price (VWAP) — institutional benchmark.
//!
//! Resets at each session (daily by default). Price > VWAP = bullish.

use super::{Candle, Indicator, IndicatorValue};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub struct Vwap {
    cumulative_pv: Decimal,
    cumulative_volume: Decimal,
    reset_interval_ms: u64,
    last_ts: Option<u64>,
    count: usize,
}

impl Vwap {
    pub fn new(reset_interval_ms: u64) -> Self {
        Self { cumulative_pv: Decimal::ZERO, cumulative_volume: Decimal::ZERO, reset_interval_ms, last_ts: None, count: 0 }
    }
}

impl Indicator for Vwap {
    fn name(&self) -> &str { "vwap" }

    fn tick_candle(&mut self, c: &Candle) -> Vec<IndicatorValue> {
        self.count += 1;
        if let Some(last) = self.last_ts {
            if c.timestamp_ms - last > self.reset_interval_ms {
                self.cumulative_pv = Decimal::ZERO;
                self.cumulative_volume = Decimal::ZERO;
            }
        }
        self.last_ts = Some(c.timestamp_ms);
        let typical = (c.high + c.low + c.close) / dec!(3);
        self.cumulative_pv += typical * c.volume;
        self.cumulative_volume += c.volume;
        let vwap = if self.cumulative_volume > Decimal::ZERO {
            self.cumulative_pv / self.cumulative_volume
        } else { typical };
        vec![IndicatorValue { name: "vwap".into(), value: vwap.round_dp(2) },
              IndicatorValue { name: "value".into(), value: vwap.round_dp(2) }]
    }

    fn reset(&mut self) { self.cumulative_pv = Decimal::ZERO; self.cumulative_volume = Decimal::ZERO; self.count = 0; }
    fn min_samples(&self) -> usize { 1 }
}
