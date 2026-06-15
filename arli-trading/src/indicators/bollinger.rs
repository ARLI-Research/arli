use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use super::{Indicator, IndicatorValue};

pub struct Bollinger {
    period: usize,
    std_dev_mult: Decimal,
    prices: Vec<Decimal>,
}

impl Bollinger {
    pub fn new(period: usize, std_dev_mult: Decimal) -> Self {
        Self { period, std_dev_mult, prices: Vec::new() }
    }
}

fn sqrt_approx(x: Decimal) -> Decimal {
    if x <= Decimal::ZERO { return Decimal::ZERO; }
    let mut guess = x / dec!(2);
    for _ in 0..10 { guess = (guess + x / guess) / dec!(2); }
    guess
}

impl Indicator for Bollinger {
    fn name(&self) -> &str { "bollinger" }

    fn tick(&mut self, price: Decimal, _ts: u64) -> Vec<IndicatorValue> {
        self.prices.push(price);
        if self.prices.len() > self.period { self.prices.remove(0); }
        if self.prices.len() < self.period { return vec![]; }
        let n = Decimal::from(self.period);
        let sum: Decimal = self.prices.iter().sum();
        let middle = sum / n;
        let variance: Decimal = self.prices.iter().map(|p| (p - middle) * (p - middle)).sum::<Decimal>() / n;
        let std_dev = sqrt_approx(variance);
        let upper = middle + std_dev * self.std_dev_mult;
        let lower = middle - std_dev * self.std_dev_mult;
        let bandwidth = if middle != Decimal::ZERO { (upper - lower) / middle * dec!(100) } else { Decimal::ZERO };
        vec![
            IndicatorValue { name: "bb_upper".into(), value: upper.round_dp(2) },
            IndicatorValue { name: "bb_middle".into(), value: middle.round_dp(2) },
            IndicatorValue { name: "bb_lower".into(), value: lower.round_dp(2) },
            IndicatorValue { name: "bb_bandwidth".into(), value: bandwidth.round_dp(2) },
            IndicatorValue { name: "value".into(), value: middle.round_dp(2) },
        ]
    }

    fn reset(&mut self) { self.prices.clear(); }
    fn min_samples(&self) -> usize { self.period }
}
