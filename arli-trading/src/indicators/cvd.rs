//! Cumulative Volume Delta (CVD) — net buying vs selling volume.
//!
//! Positive = buyers dominant, negative = sellers dominant.
//! Simplified: uses close position within bar as proxy for buy/sell pressure.

use super::{Candle, Indicator, IndicatorValue};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub struct Cvd {
    cumulative: Decimal,
    count: usize,
}

impl Cvd {
    pub fn new() -> Self { Self { cumulative: Decimal::ZERO, count: 0 } }
}

impl Indicator for Cvd {
    fn name(&self) -> &str { "cvd" }

    fn tick_candle(&mut self, c: &Candle) -> Vec<IndicatorValue> {
        self.count += 1;
        let range = c.high - c.low;
        let delta = if range > Decimal::ZERO {
            let bar_position = (c.close - c.low) / range;
            c.volume * (bar_position - dec!(0.5)) * dec!(2)
        } else { Decimal::ZERO };
        self.cumulative += delta;

        vec![
            IndicatorValue { name: "cvd".into(), value: self.cumulative },
            IndicatorValue { name: "cvd_delta".into(), value: delta.round_dp(2) },
            IndicatorValue { name: "value".into(), value: self.cumulative },
        ]
    }

    fn reset(&mut self) { self.cumulative = Decimal::ZERO; self.count = 0; }
    fn min_samples(&self) -> usize { 1 }
}
