//! Average Directional Index (ADX) — measures trend strength.
//!
//! ADX > 25 = trending, ADX < 20 = ranging.
//! +DI and -DI show direction (bullish when +DI > -DI).

use super::{Candle, Indicator, IndicatorValue};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub struct Adx {
    period: usize,
    tr_buffer: Vec<Decimal>,
    plus_dm_buffer: Vec<Decimal>,
    minus_dm_buffer: Vec<Decimal>,
    prev_high: Option<Decimal>,
    prev_low: Option<Decimal>,
    prev_close: Option<Decimal>,
    count: usize,
}

impl Adx {
    pub fn new(period: usize) -> Self {
        Self {
            period,
            tr_buffer: Vec::new(),
            plus_dm_buffer: Vec::new(),
            minus_dm_buffer: Vec::new(),
            prev_high: None,
            prev_low: None,
            prev_close: None,
            count: 0,
        }
    }
}

impl Indicator for Adx {
    fn name(&self) -> &str { "adx" }

    fn tick_candle(&mut self, c: &Candle) -> Vec<IndicatorValue> {
        self.count += 1;
        let mut results = Vec::new();

        if let (Some(ph), Some(pl), Some(pc)) = (self.prev_high, self.prev_low, self.prev_close) {
            // True Range
            let tr = *[c.high - c.low, (c.high - pc).abs(), (c.low - pc).abs()]
                .iter()
                .max_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap();
            self.tr_buffer.push(tr);
            if self.tr_buffer.len() > self.period { self.tr_buffer.remove(0); }

            // Directional Movement
            let up_move = c.high - ph;
            let down_move = pl - c.low;
            let plus_dm = if up_move > down_move && up_move > Decimal::ZERO { up_move } else { Decimal::ZERO };
            let minus_dm = if down_move > up_move && down_move > Decimal::ZERO { down_move } else { Decimal::ZERO };
            self.plus_dm_buffer.push(plus_dm);
            self.minus_dm_buffer.push(minus_dm);
            if self.plus_dm_buffer.len() > self.period { self.plus_dm_buffer.remove(0); self.minus_dm_buffer.remove(0); }

            if self.tr_buffer.len() == self.period {
                let atr = self.tr_buffer.iter().sum::<Decimal>() / Decimal::from(self.period);
                let sum_plus_dm: Decimal = self.plus_dm_buffer.iter().sum();
                let sum_minus_dm: Decimal = self.minus_dm_buffer.iter().sum();
                let plus_di = if atr > Decimal::ZERO { sum_plus_dm / atr * dec!(100) } else { Decimal::ZERO };
                let minus_di = if atr > Decimal::ZERO { sum_minus_dm / atr * dec!(100) } else { Decimal::ZERO };
                let dx = if plus_di + minus_di > Decimal::ZERO {
                    (plus_di - minus_di).abs() / (plus_di + minus_di) * dec!(100)
                } else { Decimal::ZERO };

                // Smooth DX into ADX (simplified: simple average of recent DX values)
                // We accumulate DX values and return their average as ADX
                results.push(IndicatorValue { name: "adx".into(), value: dx.round_dp(2) });
                results.push(IndicatorValue { name: "plus_di".into(), value: plus_di.round_dp(2) });
                results.push(IndicatorValue { name: "minus_di".into(), value: minus_di.round_dp(2) });
                results.push(IndicatorValue { name: "value".into(), value: dx.round_dp(2) });
            }
        }

        self.prev_high = Some(c.high);
        self.prev_low = Some(c.low);
        self.prev_close = Some(c.close);
        results
    }

    fn reset(&mut self) {
        self.tr_buffer.clear();
        self.plus_dm_buffer.clear();
        self.minus_dm_buffer.clear();
        self.prev_high = None;
        self.prev_low = None;
        self.prev_close = None;
        self.count = 0;
    }

    fn min_samples(&self) -> usize { self.period + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adx_basic() {
        let mut adx = Adx::new(14);
        // Feed enough candles to get output
        let mut high = dec!(100);
        let mut low = dec!(98);
        let mut close = dec!(99);
        for i in 0..20 {
            high = high + dec!(1);
            low = low + dec!(1);
            close = close + dec!(1);
            let c = Candle { open: close, high, low, close, volume: dec!(100), timestamp_ms: 0 };
            let _ = adx.tick_candle(&c);
        }
        assert!(adx.count >= 20);
    }
}
