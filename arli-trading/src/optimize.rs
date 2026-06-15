//! Parametric optimization — grid search over strategy parameters.
//!
//! Runs backtests for every parameter combination and ranks by Sharpe ratio.

use crate::backtest::{BacktestConfig, BacktestEngine, BacktestReport};
use crate::strategies::indicator_strategy::{Condition, IndicatorStrategy, IndicatorStrategyConfig};
use hypersdk::hypercore::types::Candle;
use std::collections::HashMap;

/// A single parameter grid dimension.
#[derive(Debug, Clone)]
pub enum ParamGrid {
    /// Float values to try (e.g. RSI thresholds)
    Float(Vec<f64>),
    /// String values to try (e.g. indicator periods)
    String(Vec<String>),
    /// Bool values to try
    Bool(Vec<bool>),
}

/// Optimization configuration.
#[derive(Debug, Clone)]
pub struct OptimizeConfig {
    /// Base strategy config template.
    pub template: IndicatorStrategyConfig,
    /// Parameter grids: dotted path → values to try.
    /// Supported paths:
    ///   - "entry.value" — entry threshold (f64)
    ///   - "exit.value" — exit threshold (f64)
    ///   - "entry.params.<key>" — indicator param (String)
    ///   - "exit.params.<key>" — indicator param (String)
    ///   - "stop_loss_pct" — (f64)
    ///   - "take_profit_pct" — (f64)
    ///   - "position_size_pct" — (f64)
    ///   - "max_leverage" — (u32)
    pub grid: HashMap<String, ParamGrid>,
}

/// Single optimization result.
#[derive(Debug, Clone)]
pub struct OptimizeResult {
    /// Parameter values that produced this result.
    pub params: HashMap<String, String>,
    /// Backtest report.
    pub report: BacktestReport,
}

/// Optimization summary.
#[derive(Debug, Clone)]
pub struct OptimizeSummary {
    pub total_combinations: usize,
    pub completed: usize,
    pub best_sharpe: f64,
    pub best_pnl: f64,
    pub results: Vec<OptimizeResult>,
}

/// Run grid search optimization over historical candles.
pub fn run_optimize(
    config: OptimizeConfig,
    candles: &[Candle],
    bt_config: BacktestConfig,
    progress: impl Fn(usize, usize),
) -> OptimizeSummary {
    let combinations = build_combinations(&config.grid);
    let total = combinations.len();

    let mut results: Vec<OptimizeResult> = Vec::with_capacity(total);

    for (idx, params) in combinations.iter().enumerate() {
        progress(idx + 1, total);

        let strategy_config = apply_params(&config.template, params);
        let strategy = IndicatorStrategy::new(strategy_config);

        let mut engine = BacktestEngine::new(Box::new(strategy), bt_config.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let report = rt.block_on(engine.run(candles));
        drop(rt);

        results.push(OptimizeResult {
            params: params.clone(),
            report,
        });
    }

    // Sort by Sharpe ratio descending
    results.sort_by(|a, b| {
        b.report
            .sharpe_ratio
            .partial_cmp(&a.report.sharpe_ratio)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let best_sharpe = results
        .first()
        .map(|r| r.report.sharpe_ratio.to_string().parse::<f64>().unwrap_or(0.0))
        .unwrap_or(0.0);

    let best_pnl = results
        .iter()
        .map(|r| r.report.total_pnl.to_string().parse::<f64>().unwrap_or(0.0))
        .fold(f64::NEG_INFINITY, f64::max);

    OptimizeSummary {
        total_combinations: total,
        completed: results.len(),
        best_sharpe,
        best_pnl,
        results,
    }
}

/// Build cartesian product of all parameter combinations.
fn build_combinations(grid: &HashMap<String, ParamGrid>) -> Vec<HashMap<String, String>> {
    if grid.is_empty() {
        return vec![HashMap::new()];
    }

    let mut result: Vec<HashMap<String, String>> = vec![HashMap::new()];

    for (key, values) in grid {
        let mut new_result = Vec::new();
        for existing in &result {
            match values {
                ParamGrid::Float(vals) => {
                    for v in vals {
                        let mut m = existing.clone();
                        m.insert(key.clone(), v.to_string());
                        new_result.push(m);
                    }
                }
                ParamGrid::String(vals) => {
                    for v in vals {
                        let mut m = existing.clone();
                        m.insert(key.clone(), v.clone());
                        new_result.push(m);
                    }
                }
                ParamGrid::Bool(vals) => {
                    for v in vals {
                        let mut m = existing.clone();
                        m.insert(key.clone(), v.to_string());
                        new_result.push(m);
                    }
                }
            }
        }
        result = new_result;
    }

    result
}

/// Apply parameter overrides to a strategy config template.
fn apply_params(
    template: &IndicatorStrategyConfig,
    params: &HashMap<String, String>,
) -> IndicatorStrategyConfig {
    let mut config = template.clone();

    for (key, val) in params {
        match key.as_str() {
            "position_size_pct" => {
                if let Ok(v) = val.parse::<f64>() {
                    config.position_size_pct = v;
                }
            }
            "stop_loss_pct" => {
                config.stop_loss_pct = val.parse::<f64>().ok();
            }
            "take_profit_pct" => {
                config.take_profit_pct = val.parse::<f64>().ok();
            }
            "max_leverage" => {
                if let Ok(v) = val.parse::<u32>() {
                    config.max_leverage = v;
                }
            }
            key if key.starts_with("entry.value") => {
                if let Ok(v) = val.parse::<f64>() {
                    set_condition_value(&mut config.entry, v);
                }
            }
            key if key.starts_with("exit.value") => {
                if let Ok(v) = val.parse::<f64>() {
                    set_condition_value(&mut config.exit, v);
                }
            }
            key if key.starts_with("entry.params.") => {
                let param_name = key.strip_prefix("entry.params.").unwrap();
                set_condition_param(&mut config.entry, param_name, val.clone());
            }
            key if key.starts_with("exit.params.") => {
                let param_name = key.strip_prefix("exit.params.").unwrap();
                set_condition_param(&mut config.exit, param_name, val.clone());
            }
            _ => {}
        }
    }

    config
}

fn set_condition_value(cond: &mut Condition, value: f64) {
    match cond {
        Condition::Indicator { value: v, .. } => *v = value,
        Condition::And { conditions } | Condition::Or { conditions } => {
            for c in conditions {
                set_condition_value(c, value);
            }
        }
        Condition::Not { condition } => set_condition_value(condition, value),
        _ => {}
    }
}

fn set_condition_param(cond: &mut Condition, name: &str, val: String) {
    match cond {
        Condition::Indicator { params, .. } => {
            params.insert(name.to_string(), val);
        }
        Condition::Cross { params_a, params_b, .. } => {
            params_a.insert(name.to_string(), val.clone());
            params_b.insert(name.to_string(), val);
        }
        Condition::And { conditions } | Condition::Or { conditions } => {
            for c in conditions {
                set_condition_param(c, name, val.clone());
            }
        }
        Condition::Not { condition } => set_condition_param(condition, name, val),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candles(prices: &[f64]) -> Vec<Candle> {
        prices
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let px = rust_decimal::Decimal::from_f64_retain(p).unwrap();
                Candle {
                    open_time: i as u64 * 60_000,
                    close_time: (i + 1) as u64 * 60_000,
                    coin: "BTC".into(),
                    interval: "1h".into(),
                    open: px,
                    high: px,
                    low: px,
                    close: px,
                    volume: rust_decimal::Decimal::from(100),
                    num_trades: 10,
                }
            })
            .collect()
    }

    #[test]
    fn test_build_combinations() {
        let mut grid = HashMap::new();
        grid.insert("entry.value".into(), ParamGrid::Float(vec![30.0, 40.0]));
        grid.insert("exit.value".into(), ParamGrid::Float(vec![70.0, 80.0]));

        let combos = build_combinations(&grid);
        assert_eq!(combos.len(), 4); // 2×2
    }

    #[test]
    fn test_optimize_rsi() {
        // Trending up market
        let prices: Vec<f64> = (0..100).map(|i| 60000.0 + i as f64 * 50.0).collect();
        let candles = make_candles(&prices);

        let template = IndicatorStrategyConfig {
            coins: vec!["BTC".into()],
            tick_interval_s: 3600,
            max_leverage: 3,
            position_size_pct: 0.1,
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

        let mut grid = HashMap::new();
        grid.insert("entry.value".into(), ParamGrid::Float(vec![25.0, 30.0, 35.0]));
        grid.insert("exit.value".into(), ParamGrid::Float(vec![65.0, 70.0, 75.0]));

        let opt_config = OptimizeConfig {
            template,
            grid,
        };

        let summary = run_optimize(opt_config, &candles, BacktestConfig::default(), |_, _| {});

        assert_eq!(summary.total_combinations, 9); // 3×3
        assert_eq!(summary.completed, 9);
        assert!(summary.results.len() == 9);
        // Results should be sorted by Sharpe descending
        let first = &summary.results[0];
        let last = &summary.results[8];
        assert!(
            first.report.sharpe_ratio >= last.report.sharpe_ratio,
            "Results should be sorted by Sharpe descending"
        );
    }
}
