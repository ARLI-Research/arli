//! Technical indicators for declarative strategy engine.
//!
//! Each indicator implements the [Indicator] trait, maintaining its own state
//! across ticks. Indicators are composed by [crate::strategies::IndicatorStrategy]
//! to evaluate entry/exit conditions from JSON config.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::HashMap;

/// One calculated value from an indicator at a given tick.
#[derive(Debug, Clone)]
pub struct IndicatorValue {
    pub name: String,
    pub value: Decimal,
}

/// OHLCV candle fed to indicators that need full bar data.
#[derive(Debug, Clone, Copy)]
pub struct Candle {
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
    pub timestamp_ms: u64,
}

impl Candle {
    pub fn from_price(price: Decimal, ts: u64) -> Self {
        Self { open: price, high: price, low: price, close: price, volume: Decimal::ZERO, timestamp_ms: ts }
    }
}

/// Trait for all indicators. Stateful — each call to [tick] advances
/// the indicator's internal history and produces zero or more values.
pub trait Indicator: Send + Sync {
    fn name(&self) -> &str;
    /// Simple price tick (backward-compatible). Default forwards to tick_candle.
    fn tick(&mut self, price: Decimal, timestamp_ms: u64) -> Vec<IndicatorValue> {
        self.tick_candle(&Candle::from_price(price, timestamp_ms))
    }
    /// Full OHLCV tick. Override this for indicators that need bar data.
    fn tick_candle(&mut self, candle: &Candle) -> Vec<IndicatorValue> {
        self.tick(candle.close, candle.timestamp_ms)
    }
    fn reset(&mut self);
    fn min_samples(&self) -> usize;
}

// Re-export all built-in indicators
mod rsi;
mod ema;
mod sma;
mod macd;
mod bollinger;
mod atr;
mod stochastic;
mod adx;
mod ichimoku;
mod vwap;
mod obv;
mod supertrend;
mod keltner;
mod parabolic_sar;
mod cci;
mod cvd;

pub use rsi::Rsi;
pub use ema::Ema;
pub use sma::Sma;
pub use macd::Macd;
pub use bollinger::Bollinger;
pub use atr::Atr;
pub use stochastic::Stochastic;
pub use adx::Adx;
pub use ichimoku::Ichimoku;
pub use vwap::Vwap;
pub use obv::Obv;
pub use supertrend::Supertrend;
pub use keltner::Keltner;
pub use parabolic_sar::ParabolicSar;
pub use cci::Cci;
pub use cvd::Cvd;

// ── Helpers ─────────────────────────────────────────────────────────────────

pub fn parse_period(params: &HashMap<String, String>, key: &str, default: usize) -> usize {
    params.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub fn parse_decimal(params: &HashMap<String, String>, key: &str, default: Decimal) -> Decimal {
    params.get(key).and_then(|v| v.parse::<Decimal>().ok()).unwrap_or(default)
}

/// Factory: build an indicator by name with string params.
pub fn build_indicator(name: &str, params: &HashMap<String, String>) -> Option<Box<dyn Indicator>> {
    match name {
        "rsi" => {
            let period = parse_period(params, "period", 14);
            Some(Box::new(Rsi::new(period)))
        }
        "ema" => {
            let period = parse_period(params, "period", 20);
            Some(Box::new(Ema::new(period)))
        }
        "sma" => {
            let period = parse_period(params, "period", 20);
            Some(Box::new(Sma::new(period)))
        }
        "macd" => {
            let fast = parse_period(params, "fast", 12);
            let slow = parse_period(params, "slow", 26);
            let signal = parse_period(params, "signal", 9);
            Some(Box::new(Macd::new(fast, slow, signal)))
        }
        "bollinger" => {
            let period = parse_period(params, "period", 20);
            let std_dev = parse_decimal(params, "std_dev", dec!(2));
            Some(Box::new(Bollinger::new(period, std_dev)))
        }
        "atr" => {
            let period = parse_period(params, "period", 14);
            Some(Box::new(Atr::new(period)))
        }
        "stochastic" => {
            let k_period = parse_period(params, "k_period", 14);
            let d_period = parse_period(params, "d_period", 3);
            Some(Box::new(Stochastic::new(k_period, d_period)))
        }
        "adx" => {
            let period = parse_period(params, "period", 14);
            Some(Box::new(Adx::new(period)))
        }
        "ichimoku" => {
            let tenkan = parse_period(params, "tenkan", 9);
            let kijun = parse_period(params, "kijun", 26);
            let senkou_b = parse_period(params, "senkou_b", 52);
            let displacement = parse_period(params, "displacement", 26);
            Some(Box::new(Ichimoku::new(tenkan, kijun, senkou_b, displacement)))
        }
        "vwap" => {
            let reset_ms = parse_period(params, "reset_ms", 86_400_000); // daily by default
            Some(Box::new(Vwap::new(reset_ms as u64)))
        }
        "obv" => {
            Some(Box::new(Obv::new()))
        }
        "supertrend" => {
            let period = parse_period(params, "period", 10);
            let multiplier = parse_decimal(params, "multiplier", dec!(3));
            Some(Box::new(Supertrend::new(period, multiplier)))
        }
        "keltner" => {
            let ema_period = parse_period(params, "ema_period", 20);
            let atr_period = parse_period(params, "atr_period", 10);
            let multiplier = parse_decimal(params, "multiplier", dec!(2));
            Some(Box::new(Keltner::new(ema_period, atr_period, multiplier)))
        }
        "psar" => {
            let af_start = parse_decimal(params, "af_start", dec!(0.02));
            let af_step = parse_decimal(params, "af_step", dec!(0.02));
            let af_max = parse_decimal(params, "af_max", dec!(0.2));
            Some(Box::new(ParabolicSar::new(af_start, af_step, af_max)))
        }
        "cci" => {
            let period = parse_period(params, "period", 20);
            Some(Box::new(Cci::new(period)))
        }
        "cvd" => {
            Some(Box::new(Cvd::new()))
        }
        _ => None,
    }
}
