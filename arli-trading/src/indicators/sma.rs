use rust_decimal::Decimal;
use super::{Indicator, IndicatorValue};

pub struct Sma {
    period: usize,
    prices: Vec<Decimal>,
}

impl Sma {
    pub fn new(period: usize) -> Self { Self { period, prices: Vec::new() } }
}

impl Indicator for Sma {
    fn name(&self) -> &str { "sma" }

    fn tick(&mut self, price: Decimal, _ts: u64) -> Vec<IndicatorValue> {
        self.prices.push(price);
        if self.prices.len() > self.period { self.prices.remove(0); }
        if self.prices.len() < self.period { return vec![]; }
        let sum: Decimal = self.prices.iter().sum();
        let sma = sum / Decimal::from(self.period);
        vec![IndicatorValue { name: format!("sma_{}", self.period), value: sma.round_dp(2) },
              IndicatorValue { name: "value".into(), value: sma.round_dp(2) }]
    }

    fn reset(&mut self) { self.prices.clear(); }
    fn min_samples(&self) -> usize { self.period }
}
