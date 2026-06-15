//! Supertrend — trend-following indicator using ATR.
//!
//! Green = uptrend (price closes above Supertrend), Red = downtrend.

use super::{Candle, Indicator, IndicatorValue};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub struct Supertrend {
    period: usize,
    multiplier: Decimal,
    highs: Vec<Decimal>,
    lows: Vec<Decimal>,
    closes: Vec<Decimal>,
    prev_close: Option<Decimal>,
    prev_supertrend: Option<Decimal>,
    prev_direction: Option<bool>, // true = up
    count: usize,
}

impl Supertrend {
    pub fn new(period: usize, multiplier: Decimal) -> Self {
        Self { period, multiplier, highs: Vec::new(), lows: Vec::new(), closes: Vec::new(),
               prev_close: None, prev_supertrend: None, prev_direction: None, count: 0 }
    }

    fn atr_from_buffers(&self) -> Decimal {
        if self.closes.len() < 2 { return Decimal::ZERO; }
        let trs: Vec<Decimal> = (1..self.closes.len())
            .map(|i| {
                let h = self.highs[i];
                let l = self.lows[i];
                let pc = self.closes[i - 1];
                *[h - l, (h - pc).abs(), (l - pc).abs()].iter().max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap()
            })
            .collect();
        let len = trs.len().min(self.period);
        if len == 0 { return Decimal::ZERO; }
        trs.iter().rev().take(len).sum::<Decimal>() / Decimal::from(len)
    }
}

impl Indicator for Supertrend {
    fn name(&self) -> &str { "supertrend" }

    fn tick_candle(&mut self, c: &Candle) -> Vec<IndicatorValue> {
        self.count += 1;
        self.highs.push(c.high);
        self.lows.push(c.low);
        self.closes.push(c.close);
        if self.highs.len() > self.period + 5 { self.highs.remove(0); self.lows.remove(0); self.closes.remove(0); }

        if self.closes.len() < self.period { return vec![]; }

        let atr = self.atr_from_buffers();
        if atr == Decimal::ZERO { return vec![]; }

        let hl2 = (c.high + c.low) / dec!(2);
        let basic_upper = hl2 + self.multiplier * atr;
        let basic_lower = hl2 - self.multiplier * atr;

        let (final_upper, final_lower) = if let (Some(ps), Some(_pd)) = (self.prev_supertrend, self.prev_direction) {
            let upper = if basic_upper < ps || self.prev_close.map_or(false, |pc| pc > ps) { basic_upper } else { ps };
            let lower = if basic_lower > ps || self.prev_close.map_or(false, |pc| pc < ps) { basic_lower } else { ps };
            (upper, lower)
        } else {
            (basic_upper, basic_lower)
        };

        let direction = if self.prev_supertrend.map_or(true, |ps| c.close > ps) {
            c.close > final_lower
        } else {
            c.close > final_upper
        };

        let st = if direction { final_lower } else { final_upper };
        self.prev_supertrend = Some(st);
        self.prev_direction = Some(direction);
        self.prev_close = Some(c.close);

        vec![
            IndicatorValue { name: "supertrend".into(), value: st.round_dp(2) },
            IndicatorValue { name: "supertrend_dir".into(), value: if direction { dec!(1) } else { dec!(-1) } },
            IndicatorValue { name: "value".into(), value: st.round_dp(2) },
        ]
    }

    fn reset(&mut self) { self.highs.clear(); self.lows.clear(); self.closes.clear(); self.prev_close = None; self.prev_supertrend = None; self.prev_direction = None; self.count = 0; }
    fn min_samples(&self) -> usize { self.period + 2 }
}
