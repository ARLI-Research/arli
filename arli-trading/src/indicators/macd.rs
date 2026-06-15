use rust_decimal::Decimal;
use super::{Indicator, IndicatorValue};
use super::ema::Ema;

pub struct Macd {
    fast_ema: Ema,
    slow_ema: Ema,
    signal_ema: Ema,
    signal_period: usize,
    slow_period: usize,
    count: usize,
}

impl Macd {
    pub fn new(fast: usize, slow: usize, signal: usize) -> Self {
        Self {
            fast_ema: Ema::new(fast),
            slow_ema: Ema::new(slow),
            signal_ema: Ema::new(signal),
            signal_period: signal,
            slow_period: slow,
            count: 0,
        }
    }
}

impl Indicator for Macd {
    fn name(&self) -> &str { "macd" }

    fn tick(&mut self, price: Decimal, _ts: u64) -> Vec<IndicatorValue> {
        self.count += 1;
        let fast_vals = self.fast_ema.tick(price, 0);
        let slow_vals = self.slow_ema.tick(price, 0);
        if fast_vals.is_empty() || slow_vals.is_empty() { return vec![]; }
        let macd_line = fast_vals[0].value - slow_vals[0].value;
        let signal_vals = self.signal_ema.tick(macd_line, 0);
        if signal_vals.is_empty() { return vec![]; }
        let signal_line = signal_vals[0].value;
        let histogram = macd_line - signal_line;
        vec![
            IndicatorValue { name: "macd_line".into(), value: macd_line.round_dp(4) },
            IndicatorValue { name: "signal_line".into(), value: signal_line.round_dp(4) },
            IndicatorValue { name: "histogram".into(), value: histogram.round_dp(4) },
            IndicatorValue { name: "value".into(), value: macd_line.round_dp(4) },
        ]
    }

    fn reset(&mut self) {
        self.fast_ema.reset();
        self.slow_ema.reset();
        self.signal_ema.reset();
        self.count = 0;
    }
    fn min_samples(&self) -> usize { self.slow_period + self.signal_period }
}
