//! Ichimoku Kinko Hyo — five lines showing trend, support/resistance, momentum.
//!
//! Standard periods: tenkan=9, kijun=26, senkou_b=52, displacement=26.

use super::{Candle, Indicator, IndicatorValue};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub struct Ichimoku {
    tenkan_period: usize,
    kijun_period: usize,
    senkou_b_period: usize,
    displacement: usize,
    highs: Vec<Decimal>,
    lows: Vec<Decimal>,
    count: usize,
}

impl Ichimoku {
    pub fn new(tenkan: usize, kijun: usize, senkou_b: usize, displacement: usize) -> Self {
        Self {
            tenkan_period: tenkan,
            kijun_period: kijun,
            senkou_b_period: senkou_b,
            displacement,
            highs: Vec::new(),
            lows: Vec::new(),
            count: 0,
        }
    }

    fn donchian(period: usize, highs: &[Decimal], lows: &[Decimal]) -> Option<Decimal> {
        let len = highs.len();
        if len < period { return None; }
        let slice_h = &highs[len - period..];
        let slice_l = &lows[len - period..];
        let h = slice_h.iter().max().copied().unwrap();
        let l = slice_l.iter().min().copied().unwrap();
        Some((h + l) / dec!(2))
    }
}

impl Indicator for Ichimoku {
    fn name(&self) -> &str { "ichimoku" }

    fn tick_candle(&mut self, c: &Candle) -> Vec<IndicatorValue> {
        self.count += 1;
        self.highs.push(c.high);
        self.lows.push(c.low);

        let max_period = self.senkou_b_period.max(self.kijun_period).max(self.tenkan_period);
        if self.highs.len() > max_period { self.highs.remove(0); self.lows.remove(0); }

        let mut results = Vec::new();

        if let Some(tenkan) = Self::donchian(self.tenkan_period, &self.highs, &self.lows) {
            results.push(IndicatorValue { name: "tenkan_sen".into(), value: tenkan.round_dp(2) });
            results.push(IndicatorValue { name: "value".into(), value: tenkan.round_dp(2) });
        }
        if let Some(kijun) = Self::donchian(self.kijun_period, &self.highs, &self.lows) {
            results.push(IndicatorValue { name: "kijun_sen".into(), value: kijun.round_dp(2) });
        }
        if let Some(senkou_a_val) = Self::donchian(self.tenkan_period, &self.highs, &self.lows) {
            if let Some(senkou_b_val) = Self::donchian(self.kijun_period, &self.highs, &self.lows) {
                // Senkou A = (Tenkan + Kijun) / 2, projected forward by displacement
                // We emit current value; user can shift if needed
                results.push(IndicatorValue { name: "senkou_span_a".into(), value: ((senkou_a_val + senkou_b_val) / dec!(2)).round_dp(2) });
            }
        }
        if let Some(senkou_b_val) = Self::donchian(self.senkou_b_period, &self.highs, &self.lows) {
            results.push(IndicatorValue { name: "senkou_span_b".into(), value: senkou_b_val.round_dp(2) });
        }
        // Chikou = close projected backward (equivalent to current close shifted view)
        results.push(IndicatorValue { name: "chikou_span".into(), value: c.close.round_dp(2) });

        results
    }

    fn reset(&mut self) {
        self.highs.clear();
        self.lows.clear();
        self.count = 0;
    }

    fn min_samples(&self) -> usize { self.senkou_b_period }
}
