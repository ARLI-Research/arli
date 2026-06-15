//! On-Balance Volume (OBV) — volume confirms price trends.
//!
//! Rising OBV with rising price = strong trend. Divergence = reversal signal.

use super::{Candle, Indicator, IndicatorValue};
use rust_decimal::Decimal;

pub struct Obv {
    obv: Decimal,
    prev_close: Option<Decimal>,
    count: usize,
}

impl Obv {
    pub fn new() -> Self { Self { obv: Decimal::ZERO, prev_close: None, count: 0 } }
}

impl Indicator for Obv {
    fn name(&self) -> &str { "obv" }

    fn tick_candle(&mut self, c: &Candle) -> Vec<IndicatorValue> {
        self.count += 1;
        if let Some(prev) = self.prev_close {
            if c.close > prev {
                self.obv += c.volume;
            } else if c.close < prev {
                self.obv -= c.volume;
            }
        } else {
            self.obv = c.volume;
        }
        self.prev_close = Some(c.close);
        vec![IndicatorValue { name: "obv".into(), value: self.obv },
              IndicatorValue { name: "value".into(), value: self.obv }]
    }

    fn reset(&mut self) { self.obv = Decimal::ZERO; self.prev_close = None; self.count = 0; }
    fn min_samples(&self) -> usize { 2 }
}
