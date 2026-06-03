//! Swarm coordination — task routing, specialization, and patterns.
//!
//! Extends the base Swarm with:
//! - Agent roles/specialization (trader, researcher, reviewer, executor)
//! - Task routing: route tasks to matching agents by capability
//! - Coordination patterns: fan-out, pipe, round-robin
//! - Agent affinity: prefer agents with lower load

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{AgentHandle, AgentId};

// ── Agent Role / Specialization ──────────────────────────────────────────

/// Agent role determines what kind of tasks the agent handles.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    /// Trading execution, strategy backtesting
    Trader,
    /// Web research, data gathering
    Researcher,
    /// Code review, security audit
    Reviewer,
    /// General task execution
    Executor,
    /// Multi-purpose / fallback
    General,
}

impl AgentRole {
    pub fn as_str(&self) -> &str {
        match self {
            AgentRole::Trader => "trader",
            AgentRole::Researcher => "researcher",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Executor => "executor",
            AgentRole::General => "general",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "trader" => AgentRole::Trader,
            "researcher" => AgentRole::Researcher,
            "reviewer" => AgentRole::Reviewer,
            "executor" => AgentRole::Executor,
            _ => AgentRole::General,
        }
    }
}

// ── Task ──────────────────────────────────────────────────────────────────

/// A task to be routed to an agent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SwarmTask {
    pub id: String,
    /// Target role (if any)
    pub target_role: Option<AgentRole>,
    /// Preferred agent ID (if any)
    pub target_agent: Option<AgentId>,
    /// Task description / prompt
    pub description: String,
    /// Priority (higher = more urgent)
    pub priority: u8,
    /// Created timestamp (ISO 8601)
    pub created_at: String,
}

/// Result of routing a task.
#[derive(Debug, Clone)]
pub struct RouteResult {
    pub task_id: String,
    pub agent_id: AgentId,
    pub agent_name: String,
    pub agent_role: AgentRole,
}

// ── Agent Registry Entry ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct AgentEntry {
    handle: AgentHandle,
    role: AgentRole,
    /// Current task count (for load balancing)
    task_count: usize,
}

// ── Task Router ──────────────────────────────────────────────────────────

/// Routes incoming tasks to agents based on role and load.
pub struct TaskRouter {
    agents: RwLock<HashMap<AgentId, AgentEntry>>,
    /// Round-robin index per role
    round_robin: RwLock<HashMap<AgentRole, usize>>,
}

impl TaskRouter {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            round_robin: RwLock::new(HashMap::new()),
        }
    }

    /// Register an agent with the router.
    pub async fn register(&self, id: AgentId, handle: AgentHandle, role: AgentRole) {
        let mut agents = self.agents.write().await;
        agents.insert(
            id,
            AgentEntry {
                handle,
                role,
                task_count: 0,
            },
        );
    }

    /// Unregister an agent.
    pub async fn unregister(&self, id: &AgentId) {
        self.agents.write().await.remove(id);
        // Clean up round-robin state for this agent's role
        // (lazy — will reset naturally)
    }

    /// Route a task to the best matching agent.
    ///
    /// Strategy (in order):
    /// 1. If target_agent is specified and available, route to it
    /// 2. If target_role is specified, pick least-loaded agent with that role
    /// 3. Fall back to least-loaded agent of any role
    pub async fn route(&self, task: &SwarmTask) -> Option<RouteResult> {
        let agents = self.agents.read().await;

        if agents.is_empty() {
            return None;
        }

        // 1. Direct targeting
        if let Some(ref target_id) = task.target_agent {
            if let Some(entry) = agents.get(target_id) {
                return Some(RouteResult {
                    task_id: task.id.clone(),
                    agent_id: target_id.clone(),
                    agent_name: entry.handle.name.clone(),
                    agent_role: entry.role.clone(),
                });
            }
        }

        // 2. Role-based routing — least loaded
        if let Some(ref role) = task.target_role {
            let candidates: Vec<&AgentEntry> =
                agents.values().filter(|e| e.role == *role).collect();

            if !candidates.is_empty() {
                // Pick least-loaded
                let best = candidates.iter().min_by_key(|e| e.task_count).unwrap();
                return Some(RouteResult {
                    task_id: task.id.clone(),
                    agent_id: best.handle.id.clone(),
                    agent_name: best.handle.name.clone(),
                    agent_role: best.role.clone(),
                });
            }
        }

        // 3. Fallback — least-loaded of any role
        let best = agents.values().min_by_key(|e| e.task_count).unwrap();

        Some(RouteResult {
            task_id: task.id.clone(),
            agent_id: best.handle.id.clone(),
            agent_name: best.handle.name.clone(),
            agent_role: best.role.clone(),
        })
    }

    /// Route with round-robin strategy (per role).
    pub async fn route_round_robin(&self, role: AgentRole) -> Option<(AgentId, String)> {
        let agents = self.agents.read().await;
        let mut rr = self.round_robin.write().await;

        let candidates: Vec<&AgentId> = agents
            .iter()
            .filter(|(_, e)| e.role == role)
            .map(|(id, _)| id)
            .collect();

        if candidates.is_empty() {
            return None;
        }

        let idx = rr.entry(role.clone()).or_insert(0);
        let selected = candidates[*idx % candidates.len()].clone();
        let name = agents[&selected].handle.name.clone();
        *idx += 1;

        Some((selected, name))
    }

    /// Increment task count for an agent (call after dispatching).
    pub async fn task_started(&self, agent_id: &AgentId) {
        let mut agents = self.agents.write().await;
        if let Some(entry) = agents.get_mut(agent_id) {
            entry.task_count = entry.task_count.saturating_add(1);
        }
    }

    /// Decrement task count for an agent (call on completion).
    pub async fn task_completed(&self, agent_id: &AgentId) {
        let mut agents = self.agents.write().await;
        if let Some(entry) = agents.get_mut(agent_id) {
            entry.task_count = entry.task_count.saturating_sub(1);
        }
    }

    /// Get load stats for all agents.
    pub async fn load_stats(&self) -> Vec<(AgentId, String, AgentRole, usize)> {
        let agents = self.agents.read().await;
        agents
            .iter()
            .map(|(id, entry)| {
                (
                    id.clone(),
                    entry.handle.name.clone(),
                    entry.role.clone(),
                    entry.task_count,
                )
            })
            .collect()
    }

    /// Number of registered agents.
    pub async fn agent_count(&self) -> usize {
        self.agents.read().await.len()
    }

    /// Count agents by role.
    pub async fn count_by_role(&self, role: &AgentRole) -> usize {
        self.agents
            .read()
            .await
            .values()
            .filter(|e| e.role == *role)
            .count()
    }
}

impl Default for TaskRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ── Coordination Patterns ────────────────────────────────────────────────

/// High-level coordination patterns for multi-agent workflows.
pub struct CoordinationPatterns {
    router: Arc<TaskRouter>,
}

impl CoordinationPatterns {
    pub fn new(router: Arc<TaskRouter>) -> Self {
        Self { router }
    }

    /// Fan-out: send the same task to ALL agents of a given role.
    /// Returns all route results.
    pub async fn fan_out(&self, task: &SwarmTask, role: AgentRole) -> Vec<RouteResult> {
        let agents = self.router.agents.read().await;
        agents
            .iter()
            .filter(|(_, e)| e.role == role)
            .map(|(id, entry)| RouteResult {
                task_id: task.id.clone(),
                agent_id: id.clone(),
                agent_name: entry.handle.name.clone(),
                agent_role: entry.role.clone(),
            })
            .collect()
    }

    /// Pipe: one agent's output becomes the next agent's input.
    /// Returns the chain of agents (ordered).
    pub fn pipe_chain(&self, roles: Vec<AgentRole>) -> Vec<AgentRole> {
        roles
    }

    /// Best-effort routing: try role-based, fall back to any available.
    pub async fn best_effort(&self, task: &SwarmTask) -> Option<RouteResult> {
        self.router.route(task).await
    }
}

// ── Swarm Task Queue ──────────────────────────────────────────────────────

/// Shared task queue with automatic distribution.
pub struct SwarmTaskQueue {
    tasks: RwLock<Vec<SwarmTask>>,
    router: Arc<TaskRouter>,
}

impl SwarmTaskQueue {
    pub fn new(router: Arc<TaskRouter>) -> Self {
        Self {
            tasks: RwLock::new(Vec::new()),
            router,
        }
    }

    /// Enqueue a new task.
    pub async fn enqueue(&self, task: SwarmTask) {
        self.tasks.write().await.push(task);
    }

    /// Dequeue and route the next pending task.
    /// Returns None if queue is empty.
    pub async fn dequeue(&self) -> Option<RouteResult> {
        let mut tasks = self.tasks.write().await;
        if tasks.is_empty() {
            return None;
        }

        // Sort by priority (descending), then by created_at
        tasks.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });

        let task = tasks.remove(0);
        drop(tasks);

        self.router.route(&task).await
    }

    /// Queue depth.
    pub async fn depth(&self) -> usize {
        self.tasks.read().await.len()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(id: &str, role: Option<AgentRole>) -> SwarmTask {
        SwarmTask {
            id: id.into(),
            target_role: role,
            target_agent: None,
            description: "test task".into(),
            priority: 5,
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn make_handle(id: &str, name: &str) -> AgentHandle {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        AgentHandle {
            id: id.into(),
            name: name.into(),
            status: super::super::SwarmAgentStatus::Running,
            sender: tx,
        }
    }

    #[tokio::test]
    async fn test_router_role_based_routing() {
        let router = TaskRouter::new();
        router
            .register(
                "a1".into(),
                make_handle("a1", "trader-1"),
                AgentRole::Trader,
            )
            .await;
        router
            .register(
                "a2".into(),
                make_handle("a2", "researcher-1"),
                AgentRole::Researcher,
            )
            .await;

        let task = make_task("t1", Some(AgentRole::Trader));
        let result = router.route(&task).await.unwrap();
        assert_eq!(result.agent_id, "a1");
        assert_eq!(result.agent_role, AgentRole::Trader);
    }

    #[tokio::test]
    async fn test_router_fallback_when_role_unavailable() {
        let router = TaskRouter::new();
        router
            .register(
                "a1".into(),
                make_handle("a1", "general"),
                AgentRole::General,
            )
            .await;

        // Ask for trader, but only general is available — should fall back
        let task = make_task("t1", Some(AgentRole::Trader));
        let result = router.route(&task).await.unwrap();
        assert_eq!(result.agent_id, "a1");
    }

    #[tokio::test]
    async fn test_router_least_loaded() {
        let router = TaskRouter::new();
        router
            .register(
                "busy".into(),
                make_handle("busy", "busy-agent"),
                AgentRole::Executor,
            )
            .await;
        router
            .register(
                "free".into(),
                make_handle("free", "free-agent"),
                AgentRole::Executor,
            )
            .await;

        // Make "busy" have more tasks
        router.task_started(&"busy".into()).await;
        router.task_started(&"busy".into()).await;
        router.task_started(&"busy".into()).await;

        let task = make_task("t1", Some(AgentRole::Executor));
        let result = router.route(&task).await.unwrap();
        // Should route to "free" (least loaded)
        assert_eq!(result.agent_id, "free");

        router.task_completed(&"busy".into()).await;
        router.task_completed(&"busy".into()).await;
    }

    #[tokio::test]
    async fn test_router_load_stats() {
        let router = TaskRouter::new();
        router
            .register("a1".into(), make_handle("a1", "agent-1"), AgentRole::Trader)
            .await;
        router.task_started(&"a1".into()).await;
        router.task_started(&"a1".into()).await;

        let stats = router.load_stats().await;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].0, "a1");
        assert_eq!(stats[0].3, 2); // task_count
    }

    #[tokio::test]
    async fn test_router_empty_returns_none() {
        let router = TaskRouter::new();
        let task = make_task("t1", None);
        assert!(router.route(&task).await.is_none());
    }

    #[tokio::test]
    async fn test_router_direct_targeting() {
        let router = TaskRouter::new();
        router
            .register(
                "a1".into(),
                make_handle("a1", "agent-1"),
                AgentRole::General,
            )
            .await;
        router
            .register(
                "a2".into(),
                make_handle("a2", "agent-2"),
                AgentRole::General,
            )
            .await;

        let mut task = make_task("t1", None);
        task.target_agent = Some("a2".into());
        let result = router.route(&task).await.unwrap();
        assert_eq!(result.agent_id, "a2");
    }

    #[tokio::test]
    async fn test_task_queue_enqueue_dequeue() {
        let router = Arc::new(TaskRouter::new());
        router
            .register(
                "a1".into(),
                make_handle("a1", "worker"),
                AgentRole::Executor,
            )
            .await;

        let queue = SwarmTaskQueue::new(router);
        queue
            .enqueue(make_task("low-prio", Some(AgentRole::Executor)))
            .await;

        let mut high = make_task("high-prio", Some(AgentRole::Executor));
        high.priority = 10;
        queue.enqueue(high).await;

        // First dequeue should get high-prio
        let result = queue.dequeue().await.unwrap();
        assert_eq!(result.task_id, "high-prio");
        assert_eq!(queue.depth().await, 1);

        // Second dequeue gets low-prio
        let result = queue.dequeue().await.unwrap();
        assert_eq!(result.task_id, "low-prio");
        assert_eq!(queue.depth().await, 0);
    }

    #[tokio::test]
    async fn test_count_by_role() {
        let router = TaskRouter::new();
        router
            .register("a1".into(), make_handle("a1", "t1"), AgentRole::Trader)
            .await;
        router
            .register("a2".into(), make_handle("a2", "t2"), AgentRole::Trader)
            .await;
        router
            .register("a3".into(), make_handle("a3", "r1"), AgentRole::Researcher)
            .await;

        assert_eq!(router.count_by_role(&AgentRole::Trader).await, 2);
        assert_eq!(router.count_by_role(&AgentRole::Researcher).await, 1);
        assert_eq!(router.count_by_role(&AgentRole::Reviewer).await, 0);
    }

    #[tokio::test]
    async fn test_unregister() {
        let router = TaskRouter::new();
        router
            .register("a1".into(), make_handle("a1", "agent"), AgentRole::General)
            .await;

        assert_eq!(router.agent_count().await, 1);
        router.unregister(&"a1".into()).await;
        assert_eq!(router.agent_count().await, 0);
    }

    #[tokio::test]
    async fn test_fan_out() {
        let router = Arc::new(TaskRouter::new());
        router
            .register("a1".into(), make_handle("a1", "t1"), AgentRole::Trader)
            .await;
        router
            .register("a2".into(), make_handle("a2", "t2"), AgentRole::Trader)
            .await;
        router
            .register("a3".into(), make_handle("a3", "r1"), AgentRole::Researcher)
            .await;

        let patterns = CoordinationPatterns::new(router);
        let task = make_task("broadcast", None);
        let results = patterns.fan_out(&task, AgentRole::Trader).await;

        assert_eq!(results.len(), 2);
        let ids: Vec<&str> = results.iter().map(|r| r.agent_id.as_str()).collect();
        assert!(ids.contains(&"a1"));
        assert!(ids.contains(&"a2"));
        assert!(!ids.contains(&"a3")); // Researcher, not Trader
    }

    #[tokio::test]
    async fn test_round_robin() {
        let router = TaskRouter::new();
        router
            .register(
                "a1".into(),
                make_handle("a1", "agent-1"),
                AgentRole::Executor,
            )
            .await;
        router
            .register(
                "a2".into(),
                make_handle("a2", "agent-2"),
                AgentRole::Executor,
            )
            .await;

        let (id1, _) = router.route_round_robin(AgentRole::Executor).await.unwrap();
        let (id2, _) = router.route_round_robin(AgentRole::Executor).await.unwrap();
        let (id3, _) = router.route_round_robin(AgentRole::Executor).await.unwrap();

        // First two should be distinct, third should equal first (wraps around)
        assert_ne!(id1, id2);
        assert_eq!(id3, id1);
        // Both agents should be in the set
        let ids = [id1.as_str(), id2.as_str()];
        assert!(ids.contains(&"a1"));
        assert!(ids.contains(&"a2"));
    }
}
