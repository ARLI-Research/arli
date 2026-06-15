use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use super::{Indicator, IndicatorValue};

pub struct Ema {
    period: usize,
    current: Option<Decimal>,
    count: usize,
    k: Decimal,
}

impl Ema {
    pub fn new(period: usize) -> Self {
        let k = dec!(2) / Decimal::from(period + 1);
        Self { period, current: None, count: 0, k }
    }
}

impl Indicator for Ema {
    fn name(&self) -> &str { "ema" }

    fn tick(&mut self, price: Decimal, _ts: u64) -> Vec<IndicatorValue> {
        self.count += 1;
        self.current = Some(match self.current {
            None => price,
            Some(prev) => (price - prev) * self.k + prev,
        });

        if self.count >= self.period {
            vec![IndicatorValue {
                name: format!("ema_{}", self.period),
                value: self.current.unwrap().round_dp(2),
            },
            IndicatorValue {
                name: "value".into(),
                value: self.current.unwrap().round_dp(2),
            }]
        } else {
            vec![]
        }
    }

    fn reset(&mut self) { self.current = None; self.count = 0; }
    fn min_samples(&self) -> usize { self.period }
}
