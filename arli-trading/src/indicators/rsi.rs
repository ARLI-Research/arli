use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use super::{Indicator, IndicatorValue};

/// Relative Strength Index (Wilder's smoothing).
pub struct Rsi {
    period: usize,
    avg_gain: Decimal,
    avg_loss: Decimal,
    prev_price: Option<Decimal>,
    count: usize,
}

impl Rsi {
    pub fn new(period: usize) -> Self {
        Self { period, avg_gain: Decimal::ZERO, avg_loss: Decimal::ZERO, prev_price: None, count: 0 }
    }
}

impl Indicator for Rsi {
    fn name(&self) -> &str { "rsi" }

    fn tick(&mut self, price: Decimal, _ts: u64) -> Vec<IndicatorValue> {
        if let Some(prev) = self.prev_price {
            let change = price - prev;
            let (gain, loss) = if change > Decimal::ZERO {
                (change, Decimal::ZERO)
            } else {
                (Decimal::ZERO, -change)
            };

            self.count += 1;
            if self.count <= self.period {
                self.avg_gain = (self.avg_gain * Decimal::from(self.count - 1) + gain) / Decimal::from(self.count);
                self.avg_loss = (self.avg_loss * Decimal::from(self.count - 1) + loss) / Decimal::from(self.count);
            } else {
                self.avg_gain = (self.avg_gain * Decimal::from(self.period - 1) + gain) / Decimal::from(self.period);
                self.avg_loss = (self.avg_loss * Decimal::from(self.period - 1) + loss) / Decimal::from(self.period);
            }
        }
        self.prev_price = Some(price);

        if self.count < self.period {
            return vec![];
        }

        let rsi = if self.avg_loss == Decimal::ZERO {
            dec!(100)
        } else {
            let rs = self.avg_gain / self.avg_loss;
            dec!(100) - (dec!(100) / (dec!(1) + rs))
        };

        vec![
            IndicatorValue {
                name: format!("rsi_{}", self.period),
                value: rsi.round_dp(2),
            },
            IndicatorValue {
                name: "value".into(),
                value: rsi.round_dp(2),
            },
        ]
    }

    fn reset(&mut self) {
        self.avg_gain = Decimal::ZERO;
        self.avg_loss = Decimal::ZERO;
        self.prev_price = None;
        self.count = 0;
    }

    fn min_samples(&self) -> usize { self.period + 1 }
}
