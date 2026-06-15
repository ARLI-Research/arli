//! Parabolic SAR — trailing stop that flips direction when price crosses.

use super::{Candle, Indicator, IndicatorValue};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub struct ParabolicSar {
    af_start: Decimal,
    af_step: Decimal,
    af_max: Decimal,
    sar: Decimal,
    ep: Decimal,        // extreme point
    af: Decimal,        // acceleration factor
    is_long: bool,
    prev_high: Option<Decimal>,
    prev_low: Option<Decimal>,
    initialized: bool,
    count: usize,
}

impl ParabolicSar {
    pub fn new(af_start: Decimal, af_step: Decimal, af_max: Decimal) -> Self {
        Self { af_start, af_step, af_max, sar: Decimal::ZERO, ep: Decimal::ZERO,
               af: af_start, is_long: true, prev_high: None, prev_low: None, initialized: false, count: 0 }
    }

    pub fn new_default() -> Self { Self::new(dec!(0.02), dec!(0.02), dec!(0.2)) }
}

impl Indicator for ParabolicSar {
    fn name(&self) -> &str { "psar" }

    fn tick_candle(&mut self, c: &Candle) -> Vec<IndicatorValue> {
        self.count += 1;
        if !self.initialized {
            self.sar = c.low;
            self.ep = c.high;
            self.is_long = true;
            self.initialized = true;
            self.prev_high = Some(c.high);
            self.prev_low = Some(c.low);
            return vec![IndicatorValue { name: "psar".into(), value: self.sar.round_dp(2) },
                        IndicatorValue { name: "psar_dir".into(), value: dec!(1) }];
        }

        if self.is_long {
            self.sar = self.sar + self.af * (self.ep - self.sar);
            if c.low < self.sar {
                // Flip to short
                self.is_long = false;
                self.sar = self.ep;
                self.ep = c.low;
                self.af = self.af_start;
            } else {
                if c.high > self.ep {
                    self.ep = c.high;
                    self.af = (self.af + self.af_step).min(self.af_max);
                }
            }
        } else {
            self.sar = self.sar - self.af * (self.sar - self.ep);
            if c.high > self.sar {
                self.is_long = true;
                self.sar = self.ep;
                self.ep = c.high;
                self.af = self.af_start;
            } else {
                if c.low < self.ep {
                    self.ep = c.low;
                    self.af = (self.af + self.af_step).min(self.af_max);
                }
            }
        }

        self.prev_high = Some(c.high);
        self.prev_low = Some(c.low);

        vec![
            IndicatorValue { name: "psar".into(), value: self.sar.round_dp(2) },
            IndicatorValue { name: "psar_dir".into(), value: if self.is_long { dec!(1) } else { dec!(-1) } },
            IndicatorValue { name: "value".into(), value: self.sar.round_dp(2) },
        ]
    }

    fn reset(&mut self) {
        self.sar = Decimal::ZERO; self.ep = Decimal::ZERO; self.af = self.af_start;
        self.is_long = true; self.initialized = false; self.prev_high = None; self.prev_low = None; self.count = 0;
    }

    fn min_samples(&self) -> usize { 2 }
}
