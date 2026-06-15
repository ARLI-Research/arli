//! Keltner Channels — volatility envelope using EMA + ATR.
//!
//! Price breaking upper band = strong bullish momentum.

use super::{Candle, Indicator, IndicatorValue};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub struct Keltner {
    ema_period: usize,
    atr_period: usize,
    multiplier: Decimal,
    prices: Vec<Decimal>,
    highs: Vec<Decimal>,
    lows: Vec<Decimal>,
    closes: Vec<Decimal>,
    count: usize,
}

impl Keltner {
    pub fn new(ema_period: usize, atr_period: usize, multiplier: Decimal) -> Self {
        Self { ema_period, atr_period, multiplier, prices: Vec::new(), highs: Vec::new(), lows: Vec::new(), closes: Vec::new(), count: 0 }
    }

    fn ema(data: &[Decimal], period: usize) -> Decimal {
        if data.len() < period { return data.last().copied().unwrap_or(Decimal::ZERO); }
        let k = dec!(2) / Decimal::from(period + 1);
        let mut ema = data[..period].iter().sum::<Decimal>() / Decimal::from(period);
        for v in &data[period..] { ema = *v * k + ema * (Decimal::ONE - k); }
        ema
    }

    fn atr_approx(&self) -> Decimal {
        if self.closes.len() < 2 { return Decimal::ZERO; }
        let mut trs = Vec::new();
        for i in 1..self.highs.len() {
            let h = self.highs[i]; let l = self.lows[i]; let pc = self.closes[i-1];
            trs.push(*[h-l, (h-pc).abs(), (l-pc).abs()].iter().max_by(|a,b| a.partial_cmp(b).unwrap()).unwrap());
        }
        let len = trs.len().min(self.atr_period);
        if len == 0 { return Decimal::ZERO; }
        trs.iter().rev().take(len).sum::<Decimal>() / Decimal::from(len)
    }
}

impl Indicator for Keltner {
    fn name(&self) -> &str { "keltner" }

    fn tick_candle(&mut self, c: &Candle) -> Vec<IndicatorValue> {
        self.count += 1;
        self.prices.push(c.close);
        self.highs.push(c.high);
        self.lows.push(c.low);
        self.closes.push(c.close);
        let max = self.ema_period.max(self.atr_period) + 5;
        if self.prices.len() > max { self.prices.remove(0); self.highs.remove(0); self.lows.remove(0); self.closes.remove(0); }

        if self.prices.len() < self.ema_period { return vec![]; }

        let middle = Self::ema(&self.prices, self.ema_period);
        let atr = self.atr_approx();
        let upper = middle + self.multiplier * atr;
        let lower = middle - self.multiplier * atr;

        vec![
            IndicatorValue { name: "kc_upper".into(), value: upper.round_dp(2) },
            IndicatorValue { name: "kc_middle".into(), value: middle.round_dp(2) },
            IndicatorValue { name: "kc_lower".into(), value: lower.round_dp(2) },
            IndicatorValue { name: "value".into(), value: middle.round_dp(2) },
        ]
    }

    fn reset(&mut self) { self.prices.clear(); self.highs.clear(); self.lows.clear(); self.closes.clear(); self.count = 0; }
    fn min_samples(&self) -> usize { self.atr_period.max(self.ema_period) + 1 }
}
