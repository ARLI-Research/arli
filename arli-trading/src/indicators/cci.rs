//! Commodity Channel Index (CCI) — unbounded momentum oscillator.
//!
//! > +100 = overbought/strong uptrend, < -100 = oversold/strong downtrend.

use super::{Candle, Indicator, IndicatorValue};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub struct Cci {
    period: usize,
    typicals: Vec<Decimal>,
    count: usize,
}

impl Cci {
    pub fn new(period: usize) -> Self { Self { period, typicals: Vec::new(), count: 0 } }
}

impl Indicator for Cci {
    fn name(&self) -> &str { "cci" }

    fn tick_candle(&mut self, c: &Candle) -> Vec<IndicatorValue> {
        self.count += 1;
        let tp = (c.high + c.low + c.close) / dec!(3);
        self.typicals.push(tp);
        if self.typicals.len() > self.period { self.typicals.remove(0); }
        if self.typicals.len() < self.period { return vec![]; }

        let sma = self.typicals.iter().sum::<Decimal>() / Decimal::from(self.period);
        let mean_dev = self.typicals.iter().map(|v| (v - sma).abs()).sum::<Decimal>() / Decimal::from(self.period);
        let cci = if mean_dev > Decimal::ZERO {
            (tp - sma) / (mean_dev * dec!(0.015)) * dec!(100)
        } else { Decimal::ZERO };

        vec![IndicatorValue { name: "cci".into(), value: cci.round_dp(2) },
              IndicatorValue { name: "value".into(), value: cci.round_dp(2) }]
    }

    fn reset(&mut self) { self.typicals.clear(); self.count = 0; }
    fn min_samples(&self) -> usize { self.period }
}
