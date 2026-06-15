use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use super::{Indicator, IndicatorValue, sma::Sma};

pub struct Stochastic {
    k_period: usize,
    d_period: usize,
    highs: Vec<Decimal>,
    lows: Vec<Decimal>,
    closes: Vec<Decimal>,
    k_values: Vec<Decimal>,
    d_sma: Sma,
}

impl Stochastic {
    pub fn new(k_period: usize, d_period: usize) -> Self {
        Self {
            k_period, d_period,
            highs: Vec::new(), lows: Vec::new(), closes: Vec::new(),
            k_values: Vec::new(),
            d_sma: Sma::new(d_period),
        }
    }
}

impl Indicator for Stochastic {
    fn name(&self) -> &str { "stochastic" }

    fn tick(&mut self, price: Decimal, _ts: u64) -> Vec<IndicatorValue> {
        // Simplified: uses close as proxy for high/low
        self.highs.push(price);
        self.lows.push(price);
        self.closes.push(price);
        if self.highs.len() > self.k_period { self.highs.remove(0); self.lows.remove(0); self.closes.remove(0); }
        if self.highs.len() < self.k_period { return vec![]; }

        let highest = self.highs.iter().max().copied().unwrap();
        let lowest = self.lows.iter().min().copied().unwrap();
        let close = self.closes.last().copied().unwrap();
        let range = highest - lowest;

        let k = if range == Decimal::ZERO { dec!(50) } else { (close - lowest) / range * dec!(100) };
        self.k_values.push(k);
        if self.k_values.len() > self.d_period { self.k_values.remove(0); }

        let d_values = self.d_sma.tick(k, 0);
        if d_values.is_empty() { return vec![]; }

        vec![
            IndicatorValue { name: "stoch_k".into(), value: k.round_dp(2) },
            IndicatorValue { name: "stoch_d".into(), value: d_values[0].value.round_dp(2) },
            IndicatorValue { name: "value".into(), value: k.round_dp(2) },
        ]
    }

    fn reset(&mut self) {
        self.highs.clear(); self.lows.clear(); self.closes.clear(); self.k_values.clear();
        self.d_sma.reset();
    }
    fn min_samples(&self) -> usize { self.k_period + self.d_period }
}
