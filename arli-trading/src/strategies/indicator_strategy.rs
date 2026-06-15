//! Declarative indicator-based strategy.
//!
//! Reads its configuration from JSON (ENS contract job_params).
//! Traders define entry/exit conditions using composable indicators.
//!
//! Example config:
//! ```json
//! {
//!   "strategy": "indicator",
//!   "coins": ["BTC", "ETH"],
//!   "entry": {"type": "indicator", "indicator": "rsi", "params": {"period": "14"}, "op": "lt", "value": 30},
//!   "exit": {"type": "indicator", "indicator": "rsi", "params": {"period": "14"}, "op": "gt", "value": 70}
//! }
//! ```

use crate::indicators::{self, Candle, Indicator};
use crate::strategy::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;

// ── Condition AST ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum Condition {
    #[serde(rename = "indicator")]
    Indicator {
        indicator: String,
        params: HashMap<String, String>,
        #[serde(default)]
        output: Option<String>,
        op: String,
        value: f64,
    },
    #[serde(rename = "cross")]
    Cross {
        indicator_a: String,
        params_a: HashMap<String, String>,
        #[serde(default)]
        output_a: Option<String>,
        indicator_b: String,
        params_b: HashMap<String, String>,
        #[serde(default)]
        output_b: Option<String>,
    },
    #[serde(rename = "and")]
    And { conditions: Vec<Condition> },
    #[serde(rename = "or")]
    Or { conditions: Vec<Condition> },
    #[serde(rename = "not")]
    Not { condition: Box<Condition> },
}

// ── Config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct IndicatorStrategyConfig {
    pub coins: Vec<String>,
    #[serde(default = "default_tick")]
    pub tick_interval_s: u64,
    #[serde(default = "default_lev")]
    pub max_leverage: u32,
    #[serde(default = "default_pct")]
    pub position_size_pct: f64,
    #[serde(default)]
    pub stop_loss_pct: Option<f64>,
    #[serde(default)]
    pub take_profit_pct: Option<f64>,
    /// Trade direction: "long", "short", or "both". Default: "long".
    #[serde(default = "default_direction")]
    pub direction: String,
    pub entry: Condition,
    pub exit: Condition,
}

fn default_tick() -> u64 { 60 }
fn default_lev() -> u32 { 5 }
fn default_pct() -> f64 { 0.05 }
fn default_direction() -> String { "long".into() }

// ── Strategy ───────────────────────────────────────────────────────────────

pub struct IndicatorStrategy {
    config: IndicatorStrategyConfig,
    indicators: HashMap<String, HashMap<String, Mutex<Box<dyn Indicator>>>>,
    prev_values: Mutex<HashMap<String, HashMap<String, Decimal>>>,
}

impl IndicatorStrategy {
    pub fn new(config: IndicatorStrategyConfig) -> Self {
        let mut indicators: HashMap<String, HashMap<String, Mutex<Box<dyn Indicator>>>> = HashMap::new();
        for coin in &config.coins {
            let mut coin_indicators: HashMap<String, Mutex<Box<dyn Indicator>>> = HashMap::new();
            collect_indicators(&config.entry, &mut coin_indicators);
            collect_indicators(&config.exit, &mut coin_indicators);
            indicators.insert(coin.clone(), coin_indicators);
        }
        Self { config, indicators, prev_values: Mutex::new(HashMap::new()) }
    }

    fn evaluate_condition(
        &self,
        cond: &Condition,
        values: &HashMap<String, Decimal>,
        prev: &HashMap<String, Decimal>,
    ) -> bool {
        match cond {
            Condition::Indicator { indicator: _, params: _, output, op, value } => {
                let output_name = output.clone().unwrap_or_else(|| "value".into());
                let current = values.get(&output_name).copied().unwrap_or(Decimal::ZERO);
                let threshold = Decimal::from_f64_retain(*value).unwrap_or(Decimal::ZERO);
                match op.as_str() {
                    "lt" => current < threshold,
                    "gt" => current > threshold,
                    "lte" => current <= threshold,
                    "gte" => current >= threshold,
                    "cross_above" => {
                        let p = prev.get(&output_name).copied().unwrap_or(current);
                        p <= threshold && current > threshold
                    }
                    "cross_below" => {
                        let p = prev.get(&output_name).copied().unwrap_or(current);
                        p >= threshold && current < threshold
                    }
                    _ => false,
                }
            }
            Condition::Cross { output_a, output_b, .. } => {
                let name_a = output_a.clone().unwrap_or_else(|| "a".into());
                let name_b = output_b.clone().unwrap_or_else(|| "b".into());
                let va = values.get(&name_a).copied().unwrap_or(Decimal::ZERO);
                let vb = values.get(&name_b).copied().unwrap_or(Decimal::ZERO);
                let pa = prev.get(&name_a).copied().unwrap_or(va);
                let pb = prev.get(&name_b).copied().unwrap_or(vb);
                pa <= pb && va > vb
            }
            Condition::And { conditions } => {
                conditions.iter().all(|c| self.evaluate_condition(c, values, prev))
            }
            Condition::Or { conditions } => {
                conditions.iter().any(|c| self.evaluate_condition(c, values, prev))
            }
            Condition::Not { condition } => {
                !self.evaluate_condition(condition, values, prev)
            }
        }
    }
}

#[async_trait::async_trait]
impl Strategy for IndicatorStrategy {
    fn name(&self) -> &str { "indicator" }
    fn version(&self) -> &str { "1.0.0" }
    fn tick_interval_seconds(&self) -> u64 { self.config.tick_interval_s }
    fn watchlist(&self) -> &[String] { &self.config.coins }

    async fn evaluate(
        &self,
        snapshot: &MarketSnapshot,
        state: &AgentState,
        _context: &HashMap<String, String>,
    ) -> Vec<Signal> {
        let mut signals = Vec::new();

        for coin in &self.config.coins {
            let price = match snapshot.mids.get(coin.as_str()) {
                Some(p) => *p,
                None => continue,
            };
            let ts = snapshot.timestamp_ms;

            let mut values: HashMap<String, Decimal> = HashMap::new();
            if let Some(coin_indicators) = self.indicators.get(coin) {
                for (_key, indicator_mutex) in coin_indicators {
                    let mut ind = indicator_mutex.lock().unwrap();
                    let results = ind.tick_candle(&Candle::from_price(price, ts));
                    for r in results {
                        values.insert(r.name, r.value);
                    }
                }
            }

            // Skip if indicators aren't warmed up yet (no output values)
            if values.is_empty() {
                continue;
            }

            let prev_map = self.prev_values.lock().unwrap();
            let prev = prev_map.get(coin).cloned().unwrap_or_default();
            drop(prev_map);

            let has_position = state.positions.iter().any(|p| p.coin.eq_ignore_ascii_case(coin));

            let dir = match self.config.direction.as_str() {
                "short" => Direction::Short,
                _ => Direction::Long,
            };

            if has_position {
                if self.evaluate_condition(&self.config.exit, &values, &prev) {
                    signals.push(Signal {
                        coin: coin.clone(),
                        direction: dir,
                        action: SignalAction::Exit,
                        confidence: dec!(0.9),
                        trigger_price: Some(price),
                        reason: format!("{coin}: exit condition met"),
                    });
                }
            } else if self.evaluate_condition(&self.config.entry, &values, &prev) {
                signals.push(Signal {
                    coin: coin.clone(),
                    direction: dir,
                    action: SignalAction::Enter,
                    confidence: dec!(0.8),
                    trigger_price: Some(price),
                    reason: format!("{coin}: entry condition met"),
                });
            }

            self.prev_values.lock().unwrap().insert(coin.clone(), values);
        }

        signals
    }

    fn size_position(
        &self,
        signal: &Signal,
        available_capital: Decimal,
        max_leverage: u32,
    ) -> PositionSize {
        let leverage = self.config.max_leverage.min(max_leverage);
        let pct = Decimal::from_f64_retain(self.config.position_size_pct).unwrap_or(dec!(0.05));
        let size = available_capital * pct * signal.confidence;

        // Compute stop-loss / take-profit from config percentages
        let entry_price = signal.trigger_price.unwrap_or(Decimal::ZERO);
        let stop_loss = self.config.stop_loss_pct.and_then(|sl_pct| {
            let pct = Decimal::from_f64_retain(sl_pct).unwrap_or(dec!(0));
            if pct > Decimal::ZERO && entry_price > Decimal::ZERO {
                Some(match signal.direction {
                    Direction::Long => entry_price * (dec!(1) - pct),
                    Direction::Short => entry_price * (dec!(1) + pct),
                })
            } else {
                None
            }
        });
        let take_profit = self.config.take_profit_pct.and_then(|tp_pct| {
            let pct = Decimal::from_f64_retain(tp_pct).unwrap_or(dec!(0));
            if pct > Decimal::ZERO && entry_price > Decimal::ZERO {
                Some(match signal.direction {
                    Direction::Long => entry_price * (dec!(1) + pct),
                    Direction::Short => entry_price * (dec!(1) - pct),
                })
            } else {
                None
            }
        });

        PositionSize {
            size_usd: size.round_dp(2),
            leverage,
            stop_loss,
            take_profit,
            order_type: OrderKind::Market,
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn collect_indicators(
    cond: &Condition,
    map: &mut HashMap<String, Mutex<Box<dyn Indicator>>>,
) {
    match cond {
        Condition::Indicator { indicator, params, .. } => {
            let key = indicator_key(indicator, params);
            if !map.contains_key(&key) {
                if let Some(ind) = indicators::build_indicator(indicator, params) {
                    map.insert(key, Mutex::new(ind));
                }
            }
        }
        Condition::Cross { indicator_a, params_a, indicator_b, params_b, .. } => {
            let key_a = indicator_key(indicator_a, params_a);
            let key_b = indicator_key(indicator_b, params_b);
            if !map.contains_key(&key_a) {
                if let Some(ind) = indicators::build_indicator(indicator_a, params_a) {
                    map.insert(key_a, Mutex::new(ind));
                }
            }
            if !map.contains_key(&key_b) {
                if let Some(ind) = indicators::build_indicator(indicator_b, params_b) {
                    map.insert(key_b, Mutex::new(ind));
                }
            }
        }
        Condition::And { conditions } | Condition::Or { conditions } => {
            for c in conditions { collect_indicators(c, map); }
        }
        Condition::Not { condition } => collect_indicators(condition, map),
    }
}

fn indicator_key(name: &str, params: &HashMap<String, String>) -> String {
    let mut parts: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
    parts.sort();
    format!("{}:{}", name, parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let json = r#"{
            "strategy": "indicator",
            "coins": ["BTC"],
            "entry": {"type": "indicator", "indicator": "rsi", "params": {"period": "14"}, "op": "lt", "value": 30},
            "exit": {"type": "indicator", "indicator": "rsi", "params": {"period": "14"}, "op": "gt", "value": 70}
        }"#;
        let _cfg: IndicatorStrategyConfig = serde_json::from_str(json).unwrap();
    }

    #[test]
    fn test_position_size_regression() {
        // Bug #3: position_size_pct=0.1 with capital=1000 should yield $80 (confidence=0.8)
        let cfg = IndicatorStrategyConfig {
            coins: vec!["BTC".into()],
            tick_interval_s: 60,
            max_leverage: 3,
            position_size_pct: 0.1,
            stop_loss_pct: Some(0.05),
            take_profit_pct: Some(0.10),
            direction: "long".into(),
            entry: Condition::Indicator {
                indicator: "rsi".into(),
                params: [("period".into(), "14".into())].into(),
                output: None,
                op: "lt".into(),
                value: 30.0,
            },
            exit: Condition::Indicator {
                indicator: "rsi".into(),
                params: [("period".into(), "14".into())].into(),
                output: None,
                op: "gt".into(),
                value: 70.0,
            },
        };
        let strategy = IndicatorStrategy::new(cfg);

        let signal = Signal {
            coin: "BTC".into(),
            direction: Direction::Long,
            action: SignalAction::Enter,
            confidence: dec!(0.8),
            trigger_price: Some(dec!(65000)),
            reason: "test".into(),
        };

        let size = strategy.size_position(&signal, dec!(1000), 50);
        assert!(size.size_usd > dec!(0), "Position size must be > 0, got {}", size.size_usd);
        assert_eq!(size.size_usd, dec!(80.00), "1000 * 0.1 * 0.8 = 80.00");
        assert_eq!(size.leverage, 3);
        // Stop-loss at 5% below entry for long: 65000 * 0.95 ≈ 61750
        assert!(size.stop_loss.is_some(), "Stop-loss must be set");
        let sl = size.stop_loss.unwrap();
        let expected_sl = dec!(61750);
        let diff = (sl - expected_sl).abs();
        assert!(diff < dec!(1), "Stop-loss {} ≈ {} (diff={})", sl, expected_sl, diff);
        // Take-profit at 10% above entry for long: 65000 * 1.10 ≈ 71500
        assert!(size.take_profit.is_some(), "Take-profit must be set");
        let tp = size.take_profit.unwrap();
        let expected_tp = dec!(71500);
        let diff = (tp - expected_tp).abs();
        assert!(diff < dec!(1), "Take-profit {} ≈ {} (diff={})", tp, expected_tp, diff);
    }

    #[test]
    fn test_position_size_default_pct() {
        // Without explicit position_size_pct, defaults to 0.05
        let cfg = IndicatorStrategyConfig {
            coins: vec!["BTC".into()],
            tick_interval_s: 60,
            max_leverage: 3,
            position_size_pct: 0.05, // default
            stop_loss_pct: None,
            take_profit_pct: None,
            direction: "long".into(),
            entry: Condition::Indicator {
                indicator: "rsi".into(),
                params: [("period".into(), "14".into())].into(),
                output: None,
                op: "lt".into(),
                value: 30.0,
            },
            exit: Condition::Indicator {
                indicator: "rsi".into(),
                params: [("period".into(), "14".into())].into(),
                output: None,
                op: "gt".into(),
                value: 70.0,
            },
        };
        let strategy = IndicatorStrategy::new(cfg);

        let signal = Signal {
            coin: "BTC".into(),
            direction: Direction::Long,
            action: SignalAction::Enter,
            confidence: dec!(0.8),
            trigger_price: None,
            reason: "test".into(),
        };

        let size = strategy.size_position(&signal, dec!(1000), 50);
        assert_eq!(size.size_usd, dec!(40.00), "1000 * 0.05 * 0.8 = 40.00");
        assert!(size.stop_loss.is_none(), "No stop-loss when trigger_price is None");
    }
}
