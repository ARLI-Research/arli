use rust_decimal::Decimal;
use super::{Indicator, IndicatorValue};

pub struct Atr {
    period: usize,
    prev_close: Option<Decimal>,
    true_ranges: Vec<Decimal>,
    current_atr: Option<Decimal>,
    count: usize,
}

impl Atr {
    pub fn new(period: usize) -> Self {
        Self { period, prev_close: None, true_ranges: Vec::new(), current_atr: None, count: 0 }
    }
}

impl Indicator for Atr {
    fn name(&self) -> &str { "atr" }

    fn tick(&mut self, price: Decimal, _ts: u64) -> Vec<IndicatorValue> {
        self.count += 1;
        if let Some(prev) = self.prev_close {
            let tr = (price - prev).abs(); // simplified: close-to-close; full ATR needs HLC
            self.true_ranges.push(tr);
            if self.true_ranges.len() > self.period { self.true_ranges.remove(0); }
        }
        self.prev_close = Some(price);

        if self.true_ranges.len() < self.period { return vec![]; }

        if self.true_ranges.len() == self.period {
            self.current_atr = Some(self.true_ranges.iter().sum::<Decimal>() / Decimal::from(self.period));
        } else {
            let prev_atr = self.current_atr.unwrap();
            let latest_tr = self.true_ranges.last().copied().unwrap();
            self.current_atr = Some((prev_atr * Decimal::from(self.period - 1) + latest_tr) / Decimal::from(self.period));
        }

        vec![IndicatorValue { name: format!("atr_{}", self.period), value: self.current_atr.unwrap().round_dp(4) },
              IndicatorValue { name: "value".into(), value: self.current_atr.unwrap().round_dp(4) }]
    }

    fn reset(&mut self) {
        self.prev_close = None;
        self.true_ranges.clear();
        self.current_atr = None;
        self.count = 0;
    }
    fn min_samples(&self) -> usize { self.period + 1 }
}
