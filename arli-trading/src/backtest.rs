//! Backtesting engine for strategy evaluation on historical data.
//!
//! Feeds historical Hyperliquid candles through a strategy,
//! simulating execution and tracking PnL.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let engine = BacktestEngine::new(strategy, config);
//! let report = engine.run(&candles).await;
//! println!("Sharpe: {:.2}", report.sharpe_ratio);
//! ```

use crate::strategy::*;
use hypersdk::hypercore::types::Candle as HlCandle;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::HashMap;

// ── Backtest Config ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BacktestConfig {
    /// Starting capital in USD.
    pub initial_capital: Decimal,
    /// Max positions (same as live).
    pub max_positions: usize,
    /// Max leverage for position sizing.
    pub max_leverage: u32,
    /// Fee per trade (e.g., 0.00035 = 3.5 bps taker).
    pub fee_rate: Decimal,
    /// Slippage in % of price (0.0005 = 5 bps).
    pub slippage: Decimal,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            initial_capital: dec!(1000),
            max_positions: 3,
            max_leverage: 50,
            fee_rate: Decimal::new(35, 5),  // 0.00035
            slippage: Decimal::new(5, 4),    // 0.0005
        }
    }
}

// ── Backtest Position ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct BtPosition {
    coin: String,
    size_usd: Decimal,
    entry_price: Decimal,
    leverage: u32,
    stop_loss: Option<Decimal>,
    take_profit: Option<Decimal>,
}

// ── Trade Record ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Trade {
    pub coin: String,
    pub direction: String,
    pub entry_price: Decimal,
    pub exit_price: Decimal,
    pub size_usd: Decimal,
    pub pnl: Decimal,
    pub pnl_pct: Decimal,
    pub entry_tick: u64,
    pub exit_tick: u64,
}

// ── Performance Report ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BacktestReport {
    /// Total number of ticks processed.
    pub total_ticks: u64,
    /// Number of trades executed.
    pub total_trades: u64,
    /// Number of winning trades.
    pub winning_trades: u64,
    /// Win rate (0.0–1.0).
    pub win_rate: Decimal,
    /// Total PnL in USD.
    pub total_pnl: Decimal,
    /// Final equity.
    pub final_equity: Decimal,
    /// Return on capital (0.0–1.0).
    pub return_on_capital: Decimal,
    /// Maximum drawdown (0.0–1.0).
    pub max_drawdown: Decimal,
    /// Sharpe ratio (annualized approximation).
    pub sharpe_ratio: Decimal,
    /// Individual trade records.
    pub trades: Vec<Trade>,
    /// Equity curve (tick, equity).
    pub equity_curve: Vec<(u64, Decimal)>,
}

// ── Engine ──────────────────────────────────────────────────────────────────

pub struct BacktestEngine {
    strategy: Box<dyn Strategy>,
    config: BacktestConfig,
}

impl BacktestEngine {
    pub fn new(strategy: Box<dyn Strategy>, config: BacktestConfig) -> Self {
        Self { strategy, config }
    }

    /// Run backtest over historical candles.
    ///
    /// Converts each HL candle into a MarketSnapshot with mid=close,
    /// feeds through the strategy, simulates fills, tracks PnL.
    pub async fn run(&mut self, candles: &[HlCandle]) -> BacktestReport {
        let mut equity = self.config.initial_capital;
        let mut peak_equity = equity;
        let mut max_drawdown = Decimal::ZERO;
        let mut positions: Vec<BtPosition> = Vec::new();
        let mut trades: Vec<Trade> = Vec::new();
        let mut equity_curve: Vec<(u64, Decimal)> = Vec::new();
        let context: HashMap<String, String> = HashMap::new();

        let coins: Vec<String> = self.strategy.watchlist().to_vec();

        for (tick_idx, hl_candle) in candles.iter().enumerate() {
            let tick = tick_idx as u64;

            // Convert HL candle to MarketSnapshot
            let mut mids = HashMap::new();
            for coin in &coins {
                if hl_candle.coin.eq_ignore_ascii_case(coin) {
                    mids.insert(coin.clone(), hl_candle.close);
                }
            }
            if mids.is_empty() {
                continue;
            }

            let snapshot = MarketSnapshot {
                mids,
                markets: vec![],
                funding: HashMap::new(),
                timestamp_ms: hl_candle.open_time,
            };

            // Build AgentState from simulated positions
            let mut agent_positions: Vec<PositionView> = Vec::new();
            let mut margin_used = Decimal::ZERO;
            for pos in &positions {
                let unrealized = if pos.size_usd > Decimal::ZERO && pos.entry_price > Decimal::ZERO {
                    (hl_candle.close - pos.entry_price) / pos.entry_price * pos.size_usd
                } else {
                    Decimal::ZERO
                };
                margin_used += pos.size_usd / Decimal::from(pos.leverage);
                agent_positions.push(PositionView {
                    coin: pos.coin.clone(),
                    size: pos.size_usd,
                    entry_price: pos.entry_price,
                    unrealized_pnl: unrealized,
                    leverage: pos.leverage,
                    liquidation_price: None,
                    stop_loss: pos.stop_loss,
                    take_profit: pos.take_profit,
                });
            }

            // Track equity with unrealized PnL
            let unrealized_total: Decimal = agent_positions
                .iter()
                .map(|p| p.unrealized_pnl)
                .sum();
            let current_equity = equity + unrealized_total;

            let agent_state = AgentState {
                equity: current_equity,
                margin_used,
                available_margin: current_equity - margin_used,
                total_pnl_all_time: current_equity - self.config.initial_capital,
                positions: agent_positions,
                open_orders: vec![],
                tick_count: tick,
            };

            // Evaluate strategy
            let signals = self.strategy.evaluate(&snapshot, &agent_state, &context).await;

            // Process signals with simulated execution
            for signal in &signals {
                // Check position limit
                if signal.action == SignalAction::Enter
                    && positions.len() >= self.config.max_positions
                    && !positions.iter().any(|p| p.coin.eq_ignore_ascii_case(&signal.coin))
                {
                    continue;
                }

                match signal.action {
                    SignalAction::Enter => {
                        // Skip if already have position
                        if positions.iter().any(|p| p.coin.eq_ignore_ascii_case(&signal.coin))
                        {
                            continue;
                        }

                        // Size position
                        let available = equity.min(self.config.initial_capital);
                        let size = self.strategy.size_position(
                            signal,
                            available,
                            self.config.max_leverage,
                        );

                        if size.size_usd <= Decimal::ZERO {
                            continue;
                        }

                        // Cap position size to max_capital_per_position
                        let max_single = self.config.initial_capital * rust_decimal_macros::dec!(0.2);
                        let capped_size = if size.size_usd > max_single { max_single } else { size.size_usd };
                        if capped_size < rust_decimal_macros::dec!(10) {
                            continue; // too small
                        }

                        // Simulate fill with slippage
                        let fill_price = if signal.direction == Direction::Long {
                            hl_candle.close * (dec!(1) + self.config.slippage)
                        } else {
                            hl_candle.close * (dec!(1) - self.config.slippage)
                        };

                        // Deduct fee from equity
                        let fee = capped_size * self.config.fee_rate;
                        equity -= fee;

                        positions.push(BtPosition {
                            coin: signal.coin.clone(),
                            size_usd: capped_size,
                            entry_price: fill_price,
                            leverage: size.leverage,
                            stop_loss: size.stop_loss,
                            take_profit: size.take_profit,
                        });
                    }
                    SignalAction::Exit => {
                        // Find and close position
                        if let Some(pos_idx) = positions
                            .iter()
                            .position(|p| p.coin.eq_ignore_ascii_case(&signal.coin))
                        {
                            let pos = positions.remove(pos_idx);

                            let exit_price = if signal.direction == Direction::Long {
                                hl_candle.close * (dec!(1) - self.config.slippage)
                            } else {
                                hl_candle.close * (dec!(1) + self.config.slippage)
                            };

                            let fee = pos.size_usd * self.config.fee_rate;
                            let pnl =
                                (exit_price - pos.entry_price) / pos.entry_price * pos.size_usd - fee;
                            let pnl_pct = if pos.entry_price > Decimal::ZERO {
                                (exit_price - pos.entry_price) / pos.entry_price
                            } else {
                                Decimal::ZERO
                            };

                            equity += pnl;

                            trades.push(Trade {
                                coin: signal.coin.clone(),
                                direction: signal.direction.as_str().into(),
                                entry_price: pos.entry_price,
                                exit_price,
                                size_usd: pos.size_usd,
                                pnl,
                                pnl_pct,
                                entry_tick: tick,
                                exit_tick: tick,
                            });
                        }
                    }
                    SignalAction::Adjust => {
                        // For now, skip adjustments in backtest
                    }
                }
            }

            // Check stop-loss / take-profit for existing positions
            let mut to_exit: Vec<usize> = Vec::new();
            for (i, pos) in positions.iter().enumerate() {
                let close = hl_candle.close;
                if let Some(sl) = pos.stop_loss {
                    if sl > Decimal::ZERO && close <= sl {
                        to_exit.push(i);
                        continue;
                    }
                }
                if let Some(tp) = pos.take_profit {
                    if tp > Decimal::ZERO && close >= tp {
                        to_exit.push(i);
                    }
                }
            }
            // Process exits in reverse order
            for i in to_exit.into_iter().rev() {
                let pos = positions.remove(i);
                let exit_price = hl_candle.close;
                let fee = pos.size_usd * self.config.fee_rate;
                let pnl = (exit_price - pos.entry_price) / pos.entry_price * pos.size_usd - fee;
                let pnl_pct = if pos.entry_price > Decimal::ZERO {
                    (exit_price - pos.entry_price) / pos.entry_price
                } else {
                    Decimal::ZERO
                };
                equity += pnl;
                trades.push(Trade {
                    coin: pos.coin.clone(),
                    direction: "long".into(),
                    entry_price: pos.entry_price,
                    exit_price,
                    size_usd: pos.size_usd,
                    pnl,
                    pnl_pct,
                    entry_tick: tick,
                    exit_tick: tick,
                });
            }

            // Update drawdown
            let total_equity = equity
                + positions
                    .iter()
                    .map(|p| {
                        if p.entry_price > Decimal::ZERO {
                            (hl_candle.close - p.entry_price) / p.entry_price * p.size_usd
                        } else {
                            Decimal::ZERO
                        }
                    })
                    .sum::<Decimal>();

            if total_equity > peak_equity {
                peak_equity = total_equity;
            }
            let dd = if peak_equity > Decimal::ZERO {
                (peak_equity - total_equity) / peak_equity
            } else {
                Decimal::ZERO
            };
            if dd > max_drawdown {
                max_drawdown = dd;
            }

            equity_curve.push((tick, total_equity));
        }

        // Close any remaining positions at last price
        if let Some(last_candle) = candles.last() {
            while let Some(pos) = positions.pop() {
                let exit_price = last_candle.close;
                let fee = pos.size_usd * self.config.fee_rate;
                let pnl =
                    (exit_price - pos.entry_price) / pos.entry_price * pos.size_usd - fee;
                let pnl_pct = if pos.entry_price > Decimal::ZERO {
                    (exit_price - pos.entry_price) / pos.entry_price
                } else {
                    Decimal::ZERO
                };
                equity += pnl;

                trades.push(Trade {
                    coin: pos.coin.clone(),
                    direction: "long".into(),
                    entry_price: pos.entry_price,
                    exit_price,
                    size_usd: pos.size_usd,
                    pnl,
                    pnl_pct,
                    entry_tick: candles.len() as u64 - 1,
                    exit_tick: candles.len() as u64 - 1,
                });
            }
        }

        let winning = trades.iter().filter(|t| t.pnl > Decimal::ZERO).count() as u64;
        let win_rate = if !trades.is_empty() {
            Decimal::from(winning) / Decimal::from(trades.len())
        } else {
            Decimal::ZERO
        };

        let roc = (equity - self.config.initial_capital) / self.config.initial_capital;

        // Simple Sharpe: mean(returns) / std(returns), scaled
        let sharpe = self.compute_sharpe(&equity_curve);

        BacktestReport {
            total_ticks: candles.len() as u64,
            total_trades: trades.len() as u64,
            winning_trades: winning,
            win_rate,
            total_pnl: equity - self.config.initial_capital,
            final_equity: equity,
            return_on_capital: roc,
            max_drawdown: max_drawdown,
            sharpe_ratio: sharpe,
            trades,
            equity_curve,
        }
    }

    fn compute_sharpe(&self, equity_curve: &[(u64, Decimal)]) -> Decimal {
        if equity_curve.len() < 2 {
            return Decimal::ZERO;
        }

        let returns: Vec<f64> = equity_curve
            .windows(2)
            .filter_map(|w| {
                let prev = w[0].1;
                let curr = w[1].1;
                if prev > Decimal::ZERO {
                    Some(((curr - prev) / prev)
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0))
                } else {
                    None
                }
            })
            .collect();

        if returns.len() < 2 {
            return Decimal::ZERO;
        }

        let n = returns.len() as f64;
        let mean = returns.iter().sum::<f64>() / n;
        let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;

        if variance <= 0.0 {
            return Decimal::ZERO;
        }

        let std_dev = variance.sqrt();
        let annualization = 724.0_f64.sqrt(); // sqrt(525,600 / interval_minutes)
        let sharpe = mean / std_dev * annualization;
        Decimal::from_f64_retain(sharpe).unwrap_or(Decimal::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategies::indicator_strategy::{Condition, IndicatorStrategy, IndicatorStrategyConfig};

    fn make_candles(prices: &[f64]) -> Vec<HlCandle> {
        prices
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let px = Decimal::from_f64_retain(p).unwrap();
                HlCandle {
                    open_time: i as u64 * 60_000,
                    close_time: (i + 1) as u64 * 60_000,
                    coin: "BTC".into(),
                    interval: "1h".into(),
                    open: px,
                    high: px,
                    low: px,
                    close: px,
                    volume: Decimal::from(100),
                    num_trades: 10,
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn test_backtest_no_signals() {
        // Flat market — RSI in 40-60 range → no signals
        let prices: Vec<f64> = (0..100).map(|i| 64000.0 + (i as f64 % 10.0) * 100.0).collect();
        let candles = make_candles(&prices);
        let strategy = IndicatorStrategy::new(
            IndicatorStrategyConfig {
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
            },
        );

        let config = BacktestConfig::default();
        let mut engine = BacktestEngine::new(Box::new(strategy), config);
        let report = engine.run(&candles).await;

        assert_eq!(report.total_ticks, 100);
        assert_eq!(report.total_trades, 0); // No RSI < 30 triggers
        assert_eq!(report.final_equity, dec!(1000)); // No PnL
        assert_eq!(report.total_pnl, Decimal::ZERO);
    }

    #[tokio::test]
    async fn test_backtest_trending_up() {
        // Strong uptrend: starts at 60000, climbs to 70000
        let prices: Vec<f64> = (0..100).map(|i| 60000.0 + i as f64 * 100.0).collect();
        let candles = make_candles(&prices);
        let strategy = IndicatorStrategy::new(
            IndicatorStrategyConfig {
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
            },
        );

        let config = BacktestConfig::default();
        let mut engine = BacktestEngine::new(Box::new(strategy), config);
        let report = engine.run(&candles).await;

        assert_eq!(report.total_ticks, 100);
        // Trending up: RSI stays high, no < 30 entry, so no trades
        // unless overbought exit triggers... but we start with no position
    }

    #[tokio::test]
    async fn test_backtest_mean_reversion() {
        use crate::strategies::MeanReversionStrategy;
        // Create mean-reverting price: oscillate 64000 ± 2000 with 20-tick cycle
        let prices: Vec<f64> = (0..200)
            .map(|i| {
                let phase = (i as f64) * std::f64::consts::PI * 2.0 / 20.0;
                64000.0 + 2000.0 * phase.sin()
            })
            .collect();
        let candles = make_candles(&prices);
        let mut strategy = MeanReversionStrategy::default();
        strategy.sma_period = 20;
        strategy.entry_threshold = rust_decimal::Decimal::from_f64_retain(0.5).unwrap();
        strategy.exit_threshold = rust_decimal::Decimal::from_f64_retain(0.1).unwrap();
        strategy.max_leverage = 10;
        strategy.watchlist = vec!["BTC".into()];
        strategy.tick_interval_s = 60;

        let config = BacktestConfig::default();
        let mut engine = BacktestEngine::new(Box::new(strategy), config);
        let report = engine.run(&candles).await;

        assert_eq!(report.total_ticks, 200);
        // Oscillating market with 1.5σ thresholds should trigger some trades
        assert!(report.total_trades > 0, "Mean reversion should generate trades on oscillations");
        // Over many cycles, mean reversion should be roughly breakeven to positive
        // (entries at extremes, exits near mean)
    }

    #[tokio::test]
    async fn test_backtest_trend_following() {
        use crate::strategies::TrendFollowingStrategy;
        // Strong uptrend: 60000 → 70000 over 100 ticks, then flat
        let prices: Vec<f64> = (0..150).map(|i| {
            if i < 100 {
                60000.0 + i as f64 * 100.0  // climb from 60k to 70k
            } else {
                70000.0  // flat
            }
        }).collect();
        let candles = make_candles(&prices);
        let mut strategy = TrendFollowingStrategy::default();
        strategy.fast_period = 9;
        strategy.slow_period = 21;
        strategy.min_crossover_strength = rust_decimal::Decimal::from_f64_retain(0.001).unwrap();
        strategy.max_leverage = 5;
        strategy.watchlist = vec!["BTC".into()];
        strategy.tick_interval_s = 120;
        let config = BacktestConfig::default();
        let mut engine = BacktestEngine::new(Box::new(strategy), config);
        let report = engine.run(&candles).await;

        assert_eq!(report.total_ticks, 150);
        // Strong trend should produce EMA crossover signals
        assert!(report.total_trades > 0, "Trend following should generate trades in trending market");
        // In a pure uptrend with no reversal, the strategy should be profitable
        // (enters long on bullish crossover, exits on bearish — since no reversal, stays in)
    }

    #[tokio::test]
    async fn test_backtest_multi_coin() {
        use crate::strategies::indicator_strategy::{Condition, IndicatorStrategy, IndicatorStrategyConfig};
        // Two coins: BTC trending up, ETH oscillating
        let candles: Vec<HlCandle> = (0..100)
            .map(|i| {
                let btc_px = 60000.0 + i as f64 * 50.0;  // BTC grinding up
                let eth_px = 3000.0 + (i as f64 * 20.0).sin() * 100.0;  // ETH oscillating
                HlCandle {
                    open_time: i as u64 * 60_000,
                    close_time: (i + 1) as u64 * 60_000,
                    coin: if i % 2 == 0 { "BTC".into() } else { "ETH".into() },
                    interval: "1h".into(),
                    open: Decimal::from_f64_retain(if i % 2 == 0 { btc_px } else { eth_px }).unwrap(),
                    high: Decimal::from_f64_retain(if i % 2 == 0 { btc_px } else { eth_px }).unwrap(),
                    low: Decimal::from_f64_retain(if i % 2 == 0 { btc_px } else { eth_px }).unwrap(),
                    close: Decimal::from_f64_retain(if i % 2 == 0 { btc_px } else { eth_px }).unwrap(),
                    volume: Decimal::from(100),
                    num_trades: 10,
                }
            })
            .collect();

        let strategy = IndicatorStrategy::new(IndicatorStrategyConfig {
            coins: vec!["BTC".into(), "ETH".into()],
            tick_interval_s: 3600,
            max_leverage: 5,
            position_size_pct: 0.1,
            stop_loss_pct: None,
            take_profit_pct: None,
            direction: "long".into(),
            entry: Condition::Indicator {
                indicator: "sma".into(),
                params: [("period".into(), "20".into())].into(),
                output: Some("value".into()),
                op: "lt".into(),
                value: 999999.0,  // Always below SMA → always enter
            },
            exit: Condition::Indicator {
                indicator: "sma".into(),
                params: [("period".into(), "20".into())].into(),
                output: Some("value".into()),
                op: "gt".into(),
                value: 0.0,  // Never exits
            },
        });

        let mut engine = BacktestEngine::new(Box::new(strategy), BacktestConfig::default());
        let report = engine.run(&candles).await;

        assert_eq!(report.total_ticks, 100);
        // With 2 coins, should get positions in both
        assert!(report.trades.len() >= 1, "Multi-coin should generate trades");
        eprintln!(
            "Multi-coin backtest: {} ticks, {} trades, PnL=${:.2}",
            report.total_ticks, report.total_trades, report.total_pnl
        );
    }
}
