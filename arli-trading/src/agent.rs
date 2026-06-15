//! Agent lifecycle — spawn, run, pause, retire trading agents.
//!
//! Each ARLI trading agent is an autonomous entity with:
//!   - Unique ID (ULID)
//!   - Own Hyperliquid private key + wallet address
//!   - Allocated capital
//!   - Strategy instance
//!   - Lifecycle state
//!
//! Agents are spawned by the AgentFactory, run via the execution loop,
//! and retired when unprofitable or manually stopped.

use crate::client::HyperliquidContext;
use crate::execution::{run_loop, AgentConfig};
use crate::strategy::StrategyRegistry;
use chrono::Utc;
use hypersdk::hypercore::PrivateKeySigner;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use ulid::Ulid;

// ─────────────────────────────────────────────────────────────────────────────
// Agent identity
// ─────────────────────────────────────────────────────────────────────────────

/// Identity and state of a single trading agent.
#[derive(Debug, Clone)]
pub struct Agent {
    pub id: String,
    pub address: String, // hex wallet address
    pub created_at: chrono::DateTime<Utc>,
    pub status: AgentStatus,
    pub strategy_name: String,
    pub allocated_capital: Decimal,
    pub current_equity: Option<Decimal>,
    pub total_trades: u64,
    pub total_pnl: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentStatus {
    Spawning,
    Running,
    Paused,
    Retiring,
    Retired,
    Failed,
}

impl AgentStatus {
    pub fn as_str(&self) -> &str {
        match self {
            AgentStatus::Spawning => "spawning",
            AgentStatus::Running => "running",
            AgentStatus::Paused => "paused",
            AgentStatus::Retiring => "retiring",
            AgentStatus::Retired => "retired",
            AgentStatus::Failed => "failed",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent registry — tracks all agents
// ─────────────────────────────────────────────────────────────────────────────

/// Thread-safe registry of all trading agents.
pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentRecord>>,
    running_flags: RwLock<HashMap<String, Arc<AtomicBool>>>,
}

#[derive(Clone)]
struct AgentRecord {
    agent: Agent,
    #[allow(dead_code)]
    private_key_hex: String, // stored for serialization, never logged
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            running_flags: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new agent (doesn't start it yet).
    pub async fn register(&self, agent: Agent, private_key_hex: String) {
        let id = agent.id.clone();
        self.agents.write().await.insert(
            id.clone(),
            AgentRecord {
                agent,
                private_key_hex,
            },
        );
        self.running_flags
            .write()
            .await
            .insert(id, Arc::new(AtomicBool::new(true)));
    }

    /// Get an agent by ID.
    pub async fn get(&self, id: &str) -> Option<Agent> {
        self.agents.read().await.get(id).map(|r| r.agent.clone())
    }

    /// List all agents.
    pub async fn list(&self) -> Vec<Agent> {
        self.agents
            .read()
            .await
            .values()
            .map(|r| r.agent.clone())
            .collect()
    }

    /// Update agent status.
    pub async fn set_status(&self, id: &str, status: AgentStatus) {
        if let Some(record) = self.agents.write().await.get_mut(id) {
            record.agent.status = status;
        }
    }

    /// Update agent performance metrics.
    pub async fn update_metrics(&self, id: &str, equity: Decimal, trades: u64, pnl: Decimal) {
        if let Some(record) = self.agents.write().await.get_mut(id) {
            record.agent.current_equity = Some(equity);
            record.agent.total_trades = trades;
            record.agent.total_pnl = pnl;
        }
    }

    /// Send pause signal to an agent.
    pub async fn pause(&self, id: &str) {
        if let Some(flag) = self.running_flags.read().await.get(id) {
            flag.store(false, Ordering::Relaxed);
        }
        self.set_status(id, AgentStatus::Paused).await;
    }

    /// Send stop signal to an agent (retire).
    pub async fn stop(&self, id: &str) {
        if let Some(flag) = self.running_flags.read().await.get(id) {
            flag.store(false, Ordering::Relaxed);
        }
        self.set_status(id, AgentStatus::Retired).await;
    }

    /// Get the running flag for an agent (used by execution loop).
    pub async fn running_flag(&self, id: &str) -> Option<Arc<AtomicBool>> {
        self.running_flags.read().await.get(id).cloned()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent factory — creates agents with wallets
// ─────────────────────────────────────────────────────────────────────────────

/// Factory for spawning trading agents with wallets and strategies.
pub struct AgentFactory {
    registry: Arc<AgentRegistry>,
    strategy_registry: Arc<StrategyRegistry>,
    is_testnet: bool,
}

impl AgentFactory {
    pub fn new(
        registry: Arc<AgentRegistry>,
        strategy_registry: Arc<StrategyRegistry>,
        is_testnet: bool,
    ) -> Self {
        Self {
            registry,
            strategy_registry,
            is_testnet,
        }
    }

    /// Spawn a new agent with a fresh wallet and allocated capital.
    ///
    /// Returns the agent ID. The agent starts in `Spawning` status.
    /// Call `start_agent()` to begin the execution loop.
    pub async fn spawn(
        &self,
        strategy_name: &str,
        allocated_capital: Decimal,
        _min_equity: Decimal,
        _max_daily_drawdown: Decimal,
    ) -> anyhow::Result<Agent> {
        // Generate fresh private key
        let signer = PrivateKeySigner::random();
        let address = signer.address();
        let private_key_hex = hex::encode(signer.to_bytes());

        // Verify strategy exists
        if self.strategy_registry.build(strategy_name).is_none() {
            anyhow::bail!("Unknown strategy: {}", strategy_name);
        }

        let id = Ulid::new().to_string();
        let now = Utc::now();

        let agent = Agent {
            id: id.clone(),
            address: address.to_string(),
            created_at: now,
            status: AgentStatus::Spawning,
            strategy_name: strategy_name.to_string(),
            allocated_capital,
            current_equity: None,
            total_trades: 0,
            total_pnl: Decimal::ZERO,
        };

        self.registry.register(agent.clone(), private_key_hex).await;

        tracing::info!(
            agent_id = %id,
            address = %address,
            strategy = %strategy_name,
            capital = %allocated_capital,
            "Agent spawned"
        );

        Ok(agent)
    }

    /// Start the execution loop for an agent. Returns immediately;
    /// the loop runs in a background task.
    pub async fn start(&self, agent_id: &str) -> anyhow::Result<()> {
        let record = {
            let agents = self.registry.agents.read().await;
            let record = agents
                .get(agent_id)
                .ok_or_else(|| anyhow::anyhow!("Agent not found: {}", agent_id))?;
            record.clone()
        };

        let running = self
            .registry
            .running_flag(agent_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("No running flag for agent: {}", agent_id))?;

        // Rebuild signer from stored hex key
        let signer: PrivateKeySigner = record.private_key_hex.parse()
            .map_err(|e| anyhow::anyhow!("Invalid private key: {:?}", e))?;

        let client = if self.is_testnet {
            hypersdk::hypercore::testnet()
        } else {
            hypersdk::hypercore::mainnet()
        };

        let ctx = Arc::new(HyperliquidContext {
            client: Arc::new(client),
            signer: Arc::new(signer.clone()),
            address: signer.address(),
            is_testnet: self.is_testnet,
        });

        let strategy = self
            .strategy_registry
            .build(&record.agent.strategy_name)
            .ok_or_else(|| {
                anyhow::anyhow!("Strategy not found: {}", record.agent.strategy_name)
            })?;

        let config = AgentConfig {
            agent_id: agent_id.to_string(),
            allocated_capital: record.agent.allocated_capital,
            min_equity: Decimal::new(100, 0),
            max_daily_drawdown: Decimal::new(2, 1),
            tick_interval_seconds: strategy.tick_interval_seconds(),
            max_positions: 3,
            live: false, // dry-run by default
        };

        let reg = self.registry.clone();
        let agent_id_clone = agent_id.to_string();

        self.registry
            .set_status(agent_id, AgentStatus::Running)
            .await;

        // Spawn the loop in background
        tokio::spawn(async move {
            let result = run_loop(ctx, strategy, config, running).await;

            // Update registry with final state
            reg.update_metrics(
                &agent_id_clone,
                result.peak_equity,
                result.total_trades,
                result.total_pnl,
            )
            .await;

            let final_status = if result.paused {
                AgentStatus::Paused
            } else if result.last_error.is_some() {
                AgentStatus::Failed
            } else {
                AgentStatus::Retired
            };
            reg.set_status(&agent_id_clone, final_status).await;

            tracing::info!(
                agent_id = %agent_id_clone,
                status = %final_status.as_str(),
                trades = %result.total_trades,
                pnl = %result.total_pnl,
                "Agent loop exited"
            );
        });

        Ok(())
    }
}
