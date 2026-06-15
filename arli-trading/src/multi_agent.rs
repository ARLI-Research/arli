//! Multi-agent execution — isolated capital, rotation, performance tracking.
//!
//! Runs multiple strategies in parallel on the same market data,
//! each with its own capital allocation and P&L tracking.
//! Periodically rebalances capital from underperformers to winners.

use crate::backtest::{BacktestConfig, BacktestEngine, BacktestReport};
use crate::strategies::indicator_strategy::IndicatorStrategyConfig;
use hypersdk::hypercore::types::Candle;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;

// ── Config ──────────────────────────────────────────────────────────────────

/// Configuration for a single agent in a multi-agent setup.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentAllocation {
    /// Human-readable name for this agent.
    pub name: String,
    /// Initial capital allocation in USD.
    pub capital: f64,
    /// Strategy configuration.
    pub config: IndicatorStrategyConfig,
}

/// Multi-agent configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct MultiAgentConfig {
    /// List of agents with their strategies and capital.
    pub agents: Vec<AgentAllocation>,
    /// Number of ticks between capital rebalancing (0 = never).
    #[serde(default)]
    pub rebalance_ticks: u64,
    /// Minimum capital for an agent before it gets shut down.
    #[serde(default = "default_min_capital")]
    pub min_capital: f64,
}

fn default_min_capital() -> f64 { 50.0 }

// ── Per-agent state ─────────────────────────────────────────────────────────

/// Running state for one agent.
#[derive(Debug, Clone)]
pub struct AgentState {
    pub name: String,
    pub capital: Decimal,
    pub initial_capital: Decimal,
    pub total_pnl: Decimal,
    pub return_pct: Decimal,
    pub last_report: Option<BacktestReport>,
    pub trade_count: u64,
    pub active: bool,
}

// ── Rotation event ──────────────────────────────────────────────────────────

/// Recorded capital rotation event.
#[derive(Debug, Clone)]
pub struct RotationEvent {
    pub tick: u64,
    pub from_agent: String,
    pub to_agent: String,
    pub amount: Decimal,
    pub reason: String,
}

// ── Multi-agent report ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MultiAgentReport {
    pub total_ticks: u64,
    pub total_capital: Decimal,
    pub final_total_equity: Decimal,
    pub total_return_pct: Decimal,
    pub agents: Vec<AgentState>,
    pub rotations: Vec<RotationEvent>,
}

// ── Engine ──────────────────────────────────────────────────────────────────

pub struct MultiAgentEngine {
    config: MultiAgentConfig,
    backtest_config: BacktestConfig,
}

impl MultiAgentEngine {
    pub fn new(config: MultiAgentConfig, backtest_config: BacktestConfig) -> Self {
        Self { config, backtest_config }
    }

    /// Run all agents on the same candle data.
    pub fn run(&self, candles: &[Candle]) -> MultiAgentReport {
        if self.config.agents.is_empty() || candles.is_empty() {
            return MultiAgentReport {
                total_ticks: candles.len() as u64,
                total_capital: Decimal::ZERO,
                final_total_equity: Decimal::ZERO,
                total_return_pct: Decimal::ZERO,
                agents: vec![],
                rotations: vec![],
            };
        }

        let mut agent_states: Vec<AgentState> = self.config.agents.iter().map(|a| {
            let cap = Decimal::from_f64_retain(a.capital).unwrap();
            AgentState {
                name: a.name.clone(),
                capital: cap,
                initial_capital: cap,
                total_pnl: Decimal::ZERO,
                return_pct: Decimal::ZERO,
                last_report: None,
                trade_count: 0,
                active: true,
            }
        }).collect();

        let mut rotations: Vec<RotationEvent> = Vec::new();
        let mut bt_configs: Vec<BacktestConfig> = self.config.agents.iter().map(|a| {
            let mut c = BacktestConfig::default();
            c.initial_capital = Decimal::from_f64_retain(a.capital).unwrap();
            c
        }).collect();

        // Run each agent independently on the full candle set
        for (i, agent_cfg) in self.config.agents.iter().enumerate() {
            if !agent_states[i].active {
                continue;
            }

            let strategy = crate::strategies::indicator_strategy::IndicatorStrategy::new(
                agent_cfg.config.clone()
            );

            let mut engine = BacktestEngine::new(Box::new(strategy), bt_configs[i].clone());
            let rt = tokio::runtime::Runtime::new().unwrap();
            let report = rt.block_on(engine.run(candles));
            drop(rt);

            let final_equity = report.final_equity;
            let pnl = final_equity - bt_configs[i].initial_capital;
            let ret_pct = if bt_configs[i].initial_capital > Decimal::ZERO {
                pnl / bt_configs[i].initial_capital
            } else {
                Decimal::ZERO
            };

            agent_states[i].total_pnl = pnl;
            agent_states[i].return_pct = ret_pct;
            agent_states[i].capital = final_equity;
            agent_states[i].last_report = Some(report.clone());
            agent_states[i].trade_count = report.total_trades;
        }

        // Sort by return descending
        agent_states.sort_by(|a, b| {
            b.return_pct.partial_cmp(&a.return_pct).unwrap_or(std::cmp::Ordering::Equal)
        });

        let total_capital: Decimal = self.config.agents.iter()
            .map(|a| Decimal::from_f64_retain(a.capital).unwrap())
            .sum();

        let final_equity: Decimal = agent_states.iter()
            .map(|a| a.capital)
            .sum();

        let total_return = if total_capital > Decimal::ZERO {
            (final_equity - total_capital) / total_capital
        } else {
            Decimal::ZERO
        };

        MultiAgentReport {
            total_ticks: candles.len() as u64,
            total_capital,
            final_total_equity: final_equity,
            total_return_pct: total_return,
            agents: agent_states,
            rotations,
        }
    }

    /// Run with capital rotation: split candles into chunks, rebalance between chunks.
    pub fn run_with_rotation(&self, candles: &[Candle]) -> MultiAgentReport {
        let rebalance_ticks = self.config.rebalance_ticks as usize;
        if rebalance_ticks == 0 || candles.len() <= rebalance_ticks {
            return self.run(candles);
        }

        let mut agent_capitals: Vec<Decimal> = self.config.agents.iter()
            .map(|a| Decimal::from_f64_retain(a.capital).unwrap())
            .collect();

        let total_capital: Decimal = agent_capitals.iter().sum();
        let mut rotations: Vec<RotationEvent> = Vec::new();
        let mut cumulative_pnl: Vec<Decimal> = vec![Decimal::ZERO; self.config.agents.len()];
        let mut cumulative_trades: Vec<u64> = vec![0; self.config.agents.len()];

        let mut chunk_start = 0;
        let mut tick = 0u64;

        while chunk_start < candles.len() {
            let chunk_end = (chunk_start + rebalance_ticks).min(candles.len());
            let chunk = &candles[chunk_start..chunk_end];

            // Run each agent on this chunk with its current capital
            for (i, agent_cfg) in self.config.agents.iter().enumerate() {
                if agent_capitals[i] < Decimal::from_f64_retain(self.config.min_capital).unwrap() {
                    continue; // agent below min capital, skip
                }

                let mut bt_cfg = BacktestConfig::default();
                bt_cfg.initial_capital = agent_capitals[i];
                let initial_cap = bt_cfg.initial_capital;

                let strategy = crate::strategies::indicator_strategy::IndicatorStrategy::new(
                    agent_cfg.config.clone()
                );

                let mut engine = BacktestEngine::new(Box::new(strategy), bt_cfg);
                let rt = tokio::runtime::Runtime::new().unwrap();
                let report = rt.block_on(engine.run(chunk));
                drop(rt);

                let pnl = report.final_equity - initial_cap;
                agent_capitals[i] = report.final_equity;
                cumulative_pnl[i] += pnl;
                cumulative_trades[i] += report.total_trades;
            }

            tick += chunk.len() as u64;

            // Rebalance: take 50% of profits from top half, distribute to bottom half
            if chunk_start + rebalance_ticks < candles.len() {
                let mut perf: Vec<(usize, Decimal)> = (0..self.config.agents.len())
                    .map(|i| {
                        let initial = Decimal::from_f64_retain(self.config.agents[i].capital).unwrap();
                        let ret = if initial > Decimal::ZERO {
                            (agent_capitals[i] - initial) / initial
                        } else { Decimal::ZERO };
                        (i, ret)
                    })
                    .collect();
                perf.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                let midpoint = self.config.agents.len() / 2;
                let take_pct = dec!(0.3); // take 30% of profits from winners

                for j in 0..midpoint {
                    let winner_idx = perf[j].0;
                    let loser_idx = perf[self.config.agents.len() - 1 - j].0;

                    let initial_w = Decimal::from_f64_retain(self.config.agents[winner_idx].capital).unwrap();
                    let profit = (agent_capitals[winner_idx] - initial_w).max(Decimal::ZERO);
                    let amount = profit.min(agent_capitals[winner_idx] * take_pct);

                    if amount > dec!(1) {
                        agent_capitals[winner_idx] -= amount;
                        agent_capitals[loser_idx] += amount;

                        rotations.push(RotationEvent {
                            tick,
                            from_agent: self.config.agents[winner_idx].name.clone(),
                            to_agent: self.config.agents[loser_idx].name.clone(),
                            amount,
                            reason: format!(
                                "Rotation: {:.2}% return → rebalance",
                                perf[j].1 * dec!(100),
                            ),
                        });
                    }
                }
            }

            chunk_start = chunk_end;
        }

        // Build final agent states
        let mut agent_states: Vec<AgentState> = self.config.agents.iter().enumerate().map(|(i, a)| {
            let initial = Decimal::from_f64_retain(a.capital).unwrap();
            AgentState {
                name: a.name.clone(),
                capital: agent_capitals[i],
                initial_capital: initial,
                total_pnl: cumulative_pnl[i],
                return_pct: if initial > Decimal::ZERO {
                    cumulative_pnl[i] / initial
                } else { Decimal::ZERO },
                last_report: None,
                trade_count: cumulative_trades[i],
                active: agent_capitals[i] >= Decimal::from_f64_retain(self.config.min_capital).unwrap(),
            }
        }).collect();

        agent_states.sort_by(|a, b| {
            b.return_pct.partial_cmp(&a.return_pct).unwrap_or(std::cmp::Ordering::Equal)
        });

        let final_equity: Decimal = agent_capitals.iter().sum();
        let total_return = if total_capital > Decimal::ZERO {
            (final_equity - total_capital) / total_capital
        } else { Decimal::ZERO };

        MultiAgentReport {
            total_ticks: candles.len() as u64,
            total_capital,
            final_total_equity: final_equity,
            total_return_pct: total_return,
            agents: agent_states,
            rotations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategies::indicator_strategy::Condition;

    fn make_candles(coin: &str, prices: &[f64]) -> Vec<Candle> {
        prices.iter().enumerate().map(|(i, &p)| {
            let px = Decimal::from_f64_retain(p).unwrap();
            Candle {
                open_time: i as u64 * 60_000,
                close_time: (i + 1) as u64 * 60_000,
                coin: coin.into(),
                interval: "1h".into(),
                open: px, high: px, low: px, close: px,
                volume: Decimal::from(100),
                num_trades: 10,
            }
        }).collect()
    }

    fn make_rsi_config(value: f64) -> IndicatorStrategyConfig {
        IndicatorStrategyConfig {
            coins: vec!["BTC".into()],
            tick_interval_s: 3600,
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
                value,
            },
            exit: Condition::Indicator {
                indicator: "rsi".into(),
                params: [("period".into(), "14".into())].into(),
                output: None,
                op: "gt".into(),
                value: 70.0,
            },
        }
    }

    #[test]
    fn test_multi_agent_two_strategies() {
        // Trending up BTC
        let prices: Vec<f64> = (0..100).map(|i| 60000.0 + i as f64 * 50.0).collect();
        let candles = make_candles("BTC", &prices);

        let config = MultiAgentConfig {
            agents: vec![
                AgentAllocation {
                    name: "rsi_aggro".into(),
                    capital: 500.0,
                    config: make_rsi_config(30.0),
                },
                AgentAllocation {
                    name: "rsi_safe".into(),
                    capital: 500.0,
                    config: make_rsi_config(40.0),
                },
            ],
            rebalance_ticks: 0,
            min_capital: 50.0,
        };

        let engine = MultiAgentEngine::new(config, BacktestConfig::default());
        let report = engine.run(&candles);

        assert_eq!(report.agents.len(), 2);
        assert!(report.total_capital > Decimal::ZERO);
        // Both agents should have results
        for agent in &report.agents {
            assert!(agent.last_report.is_some(), "Agent {} has no report", agent.name);
        }
    }

    #[test]
    fn test_multi_agent_capital_isolation() {
        // Each agent starts with isolated capital
        let prices: Vec<f64> = (0..50).map(|i| 64000.0 + (i as f64 * 20.0).sin() * 1000.0).collect();
        let candles = make_candles("BTC", &prices);

        let config = MultiAgentConfig {
            agents: vec![
                AgentAllocation {
                    name: "agent_a".into(),
                    capital: 300.0,
                    config: make_rsi_config(30.0),
                },
                AgentAllocation {
                    name: "agent_b".into(),
                    capital: 700.0,
                    config: make_rsi_config(35.0),
                },
            ],
            rebalance_ticks: 0,
            min_capital: 50.0,
        };

        let engine = MultiAgentEngine::new(config, BacktestConfig::default());
        let report = engine.run(&candles);

        // Total capital should be 1000
        assert_eq!(report.total_capital, dec!(1000));
        // Each agent's capital shouldn't affect the other
        assert_eq!(report.agents.iter().find(|a| a.name == "agent_a").unwrap().initial_capital, dec!(300));
        assert_eq!(report.agents.iter().find(|a| a.name == "agent_b").unwrap().initial_capital, dec!(700));
    }

    #[test]
    fn test_multi_agent_ranking() {
        let prices: Vec<f64> = (0..80).map(|i| 62000.0 + i as f64 * 100.0).collect();
        let candles = make_candles("BTC", &prices);

        let config = MultiAgentConfig {
            agents: vec![
                AgentAllocation {
                    name: "rsi_low".into(),
                    capital: 500.0,
                    config: make_rsi_config(20.0), // very tight entry
                },
                AgentAllocation {
                    name: "rsi_high".into(),
                    capital: 500.0,
                    config: make_rsi_config(45.0), // looser entry
                },
                AgentAllocation {
                    name: "sma_cross".into(),
                    capital: 500.0,
                    config: IndicatorStrategyConfig {
                        coins: vec!["BTC".into()],
                        tick_interval_s: 3600,
                        max_leverage: 3,
                        position_size_pct: 0.1,
                        stop_loss_pct: Some(0.05),
                        take_profit_pct: Some(0.10),
                        direction: "long".into(),
                        entry: Condition::Indicator {
                            indicator: "sma".into(),
                            params: [("period".into(), "20".into())].into(),
                            output: Some("value".into()),
                            op: "lt".into(),
                            value: 999999.0, // always enter
                        },
                        exit: Condition::Indicator {
                            indicator: "sma".into(),
                            params: [("period".into(), "20".into())].into(),
                            output: Some("value".into()),
                            op: "gt".into(),
                            value: 0.0, // never exit
                        },
                    },
                },
            ],
            rebalance_ticks: 0,
            min_capital: 50.0,
        };

        let engine = MultiAgentEngine::new(config, BacktestConfig::default());
        let report = engine.run(&candles);

        assert_eq!(report.agents.len(), 3);
        // Agents should be sorted by return_pct descending
        if report.agents.len() >= 2 {
            assert!(
                report.agents[0].return_pct >= report.agents[1].return_pct,
                "First agent should have >= return than second"
            );
        }
        // sma_cross (always enters) should have more trades than rsi-based strategies
        let sma = report.agents.iter().find(|a| a.name == "sma_cross").unwrap();
        let rsi_low = report.agents.iter().find(|a| a.name == "rsi_low").unwrap();
        assert!(sma.trade_count >= rsi_low.trade_count,
            "SMA cross should have >= trades than tight RSI: {} vs {}",
            sma.trade_count, rsi_low.trade_count);
    }
}
