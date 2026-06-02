//! ENSO Marketplace — RFQ → Quote → Contract flow for agent compute.
//!
//! Agents post Requests for Quote (RFQs) describing work they need done.
//! Other agents submit Quotes with price and timeline. The RFQ creator
//! accepts a quote, and a Contract is created on the ENSO canister.
//!
//! # Flow
//!
//! ```text
//! RFQ created  →  agents see it  →  submit Quotes  →  creator accepts  →  Contract on ICP
//! ```
//!
//! # Schema
//!
//! ```text
//! marketplace_rfqs     — id, creator, title, description, budget, deadline, status, ...
//! marketplace_quotes   — id, rfq_id, agent_id, price, estimated_time, ...
//! ```

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{Error, Result};

// ── Data model ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RfqStatus {
    Open,
    Quoted,
    Accepted,
    Contracted,
    Cancelled,
    Expired,
}

impl RfqStatus {
    pub fn as_str(&self) -> &str {
        match self {
            RfqStatus::Open => "open",
            RfqStatus::Quoted => "quoted",
            RfqStatus::Accepted => "accepted",
            RfqStatus::Contracted => "contracted",
            RfqStatus::Cancelled => "cancelled",
            RfqStatus::Expired => "expired",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "open" => RfqStatus::Open,
            "quoted" => RfqStatus::Quoted,
            "accepted" => RfqStatus::Accepted,
            "contracted" => RfqStatus::Contracted,
            "cancelled" => RfqStatus::Cancelled,
            "expired" => RfqStatus::Expired,
            _ => RfqStatus::Open,
        }
    }
}

/// Request for Quote — a task posted by an agent/user seeking compute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rfq {
    pub id: String,
    /// Agent ID or user who created this RFQ.
    pub creator: String,
    pub title: String,
    pub description: String,
    /// Budget in USD cents.
    pub budget_cents: u64,
    /// Deadline as ISO 8601 string.
    pub deadline: String,
    /// Required capabilities (e.g., ["trading", "hyperliquid"]).
    pub required_capabilities: Vec<String>,
    /// Required trust model (e.g., "KernelSandbox").
    pub required_trust_model: Option<String>,
    /// Required sandbox config hash (SHA-256).
    pub required_sandbox_config_hash: Option<String>,
    /// SLA metrics the winning agent must satisfy.
    pub sla_metrics: Vec<SlaRequirement>,
    pub status: RfqStatus,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlaRequirement {
    pub name: String,
    pub target: String,
    pub require_landlock: bool,
    pub require_seccomp: bool,
}

/// Quote — an agent's response to an RFQ.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub id: String,
    pub rfq_id: String,
    /// Agent ID submitting the quote.
    pub agent_id: String,
    /// Quoted price in USD cents.
    pub price_cents: u64,
    /// Estimated completion time in seconds.
    pub estimated_time_secs: u64,
    /// How the agent's capabilities match the RFQ requirements (text).
    pub capabilities_match: String,
    /// Agent's trust model.
    pub trust_model: String,
    /// Agent's sandbox config hash.
    pub sandbox_config_hash: Option<String>,
    /// Whether this quote has been accepted.
    pub accepted: bool,
    /// Contract ID (set when accepted and contract created).
    pub contract_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceStats {
    pub open_rfqs: usize,
    pub total_rfqs: usize,
    pub total_quotes: usize,
    pub contracted: usize,
    /// Total value of open RFQs in USD cents.
    pub open_value_cents: u64,
}

// ── MarketplaceStore ──────────────────────────────────────────────────────

pub struct MarketplaceStore {
    conn: Mutex<Connection>,
}

impl MarketplaceStore {
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Session(format!("Cannot create marketplace DB dir: {e}")))?;
        }

        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS marketplace_rfqs (
                id                           TEXT PRIMARY KEY,
                creator                      TEXT NOT NULL,
                title                        TEXT NOT NULL,
                description                  TEXT NOT NULL DEFAULT '',
                budget_cents                 INTEGER NOT NULL,
                deadline                     TEXT NOT NULL,
                required_capabilities        TEXT NOT NULL DEFAULT '[]',
                required_trust_model         TEXT,
                required_sandbox_config_hash TEXT,
                sla_metrics                  TEXT NOT NULL DEFAULT '[]',
                status                       TEXT NOT NULL DEFAULT 'open',
                created_at                   TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at                   TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS marketplace_quotes (
                id                   TEXT PRIMARY KEY,
                rfq_id               TEXT NOT NULL REFERENCES marketplace_rfqs(id) ON DELETE CASCADE,
                agent_id             TEXT NOT NULL,
                price_cents          INTEGER NOT NULL,
                estimated_time_secs  INTEGER NOT NULL,
                capabilities_match   TEXT NOT NULL DEFAULT '',
                trust_model          TEXT NOT NULL DEFAULT '',
                sandbox_config_hash  TEXT,
                accepted             INTEGER NOT NULL DEFAULT 0,
                contract_id          TEXT,
                created_at           TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_quotes_rfq ON marketplace_quotes(rfq_id);
            CREATE INDEX IF NOT EXISTS idx_rfqs_status ON marketplace_rfqs(status);",
        )?;
        Ok(())
    }

    // ── RFQs ──────────────────────────────────────────────────────────

    pub fn create_rfq(
        &self,
        creator: &str,
        title: &str,
        description: &str,
        budget_cents: u64,
        deadline: &str,
        required_capabilities: &[String],
        required_trust_model: Option<&str>,
        required_sandbox_config_hash: Option<&str>,
        sla_metrics: &[SlaRequirement],
    ) -> Result<Rfq> {
        let conn = self.lock();
        let id = ulid::Ulid::new().to_string();
        let caps_json = serde_json::to_string(required_capabilities).unwrap_or_default();
        let sla_json = serde_json::to_string(sla_metrics).unwrap_or_default();

        conn.execute(
            "INSERT INTO marketplace_rfqs (id, creator, title, description, budget_cents, deadline,
             required_capabilities, required_trust_model, required_sandbox_config_hash, sla_metrics)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                creator,
                title,
                description,
                budget_cents as i64,
                deadline,
                caps_json,
                required_trust_model,
                required_sandbox_config_hash,
                sla_json,
            ],
        )?;
        drop(conn);
        self.get_rfq(&id)
    }

    pub fn get_rfq(&self, id: &str) -> Result<Rfq> {
        let conn = self.lock();
        let rfq = conn.query_row(
            "SELECT id, creator, title, description, budget_cents, deadline,
                    required_capabilities, required_trust_model, required_sandbox_config_hash,
                    sla_metrics, status, created_at, updated_at
             FROM marketplace_rfqs WHERE id = ?1",
            params![id],
            |row| {
                let caps_str: String = row.get(6)?;
                let sla_str: String = row.get(9)?;
                Ok(Rfq {
                    id: row.get(0)?,
                    creator: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    budget_cents: row.get::<_, i64>(4)? as u64,
                    deadline: row.get(5)?,
                    required_capabilities: serde_json::from_str(&caps_str).unwrap_or_default(),
                    required_trust_model: row.get(7)?,
                    required_sandbox_config_hash: row.get(8)?,
                    sla_metrics: serde_json::from_str(&sla_str).unwrap_or_default(),
                    status: RfqStatus::from_str(&row.get::<_, String>(10)?),
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            },
        )?;
        Ok(rfq)
    }

    pub fn list_rfqs(&self, status: Option<&str>) -> Result<Vec<Rfq>> {
        let conn = self.lock();
        let (where_clause, param) = if let Some(s) = status {
            ("WHERE status = ?1".to_string(), Some(s.to_string()))
        } else {
            (String::new(), None)
        };

        let sql = format!(
            "SELECT id, creator, title, description, budget_cents, deadline,
                    required_capabilities, required_trust_model, required_sandbox_config_hash,
                    sla_metrics, status, created_at, updated_at
             FROM marketplace_rfqs {} ORDER BY created_at DESC",
            where_clause
        );

        let mut stmt = conn.prepare(&sql)?;

        // Collect into Vec to unify the two query_map paths
        let rfq_results: Vec<Rfq> = if let Some(ref p) = param {
            stmt.query_map(params![p], |row| parse_rfq_row(row))?
                .filter_map(|r| r.ok())
                .collect()
        } else {
            stmt.query_map([], |row| parse_rfq_row(row))?
                .filter_map(|r| r.ok())
                .collect()
        };

        Ok(rfq_results)
    }

    pub fn update_rfq_status(&self, id: &str, status: RfqStatus) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE marketplace_rfqs SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?;
        Ok(())
    }

    pub fn cancel_rfq(&self, id: &str) -> Result<()> {
        self.update_rfq_status(id, RfqStatus::Cancelled)
    }

    // ── Quotes ────────────────────────────────────────────────────────

    pub fn submit_quote(
        &self,
        rfq_id: &str,
        agent_id: &str,
        price_cents: u64,
        estimated_time_secs: u64,
        capabilities_match: &str,
        trust_model: &str,
        sandbox_config_hash: Option<&str>,
    ) -> Result<Quote> {
        let conn = self.lock();

        // Verify RFQ exists and is open
        let status_str: String = conn
            .query_row(
                "SELECT status FROM marketplace_rfqs WHERE id = ?1",
                params![rfq_id],
                |row| row.get(0),
            )
            .map_err(|_| Error::Session("RFQ not found".into()))?;

        if status_str != "open" && status_str != "quoted" {
            return Err(Error::Session(format!(
                "RFQ is not open for quotes (status: {status_str})"
            )));
        }

        let id = ulid::Ulid::new().to_string();
        conn.execute(
            "INSERT INTO marketplace_quotes (id, rfq_id, agent_id, price_cents, estimated_time_secs,
             capabilities_match, trust_model, sandbox_config_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id, rfq_id, agent_id, price_cents as i64, estimated_time_secs as i64,
                capabilities_match, trust_model, sandbox_config_hash,
            ],
        )?;

        // Update RFQ status to quoted
        if status_str == "open" {
            conn.execute(
                "UPDATE marketplace_rfqs SET status = 'quoted', updated_at = datetime('now') WHERE id = ?1",
                params![rfq_id],
            )?;
        }
        drop(conn);
        self.get_quote(&id)
    }

    pub fn get_quote(&self, id: &str) -> Result<Quote> {
        let conn = self.lock();
        let quote = conn.query_row(
            "SELECT id, rfq_id, agent_id, price_cents, estimated_time_secs,
                    capabilities_match, trust_model, sandbox_config_hash, accepted, contract_id, created_at
             FROM marketplace_quotes WHERE id = ?1",
            params![id],
            |row| Ok(Quote {
                id: row.get(0)?,
                rfq_id: row.get(1)?,
                agent_id: row.get(2)?,
                price_cents: row.get::<_, i64>(3)? as u64,
                estimated_time_secs: row.get::<_, i64>(4)? as u64,
                capabilities_match: row.get(5)?,
                trust_model: row.get(6)?,
                sandbox_config_hash: row.get(7)?,
                accepted: row.get::<_, i64>(8)? != 0,
                contract_id: row.get(9)?,
                created_at: row.get(10)?,
            }),
        )?;
        Ok(quote)
    }

    pub fn list_quotes_for_rfq(&self, rfq_id: &str) -> Result<Vec<Quote>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, rfq_id, agent_id, price_cents, estimated_time_secs,
                    capabilities_match, trust_model, sandbox_config_hash, accepted, contract_id, created_at
             FROM marketplace_quotes WHERE rfq_id = ?1 ORDER BY price_cents ASC",
        )?;
        let rows = stmt.query_map(params![rfq_id], |row| {
            Ok(Quote {
                id: row.get(0)?,
                rfq_id: row.get(1)?,
                agent_id: row.get(2)?,
                price_cents: row.get::<_, i64>(3)? as u64,
                estimated_time_secs: row.get::<_, i64>(4)? as u64,
                capabilities_match: row.get(5)?,
                trust_model: row.get(6)?,
                sandbox_config_hash: row.get(7)?,
                accepted: row.get::<_, i64>(8)? != 0,
                contract_id: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Accept a quote — marks it accepted, sets RFQ to accepted, generates a contract_id.
    pub fn accept_quote(&self, quote_id: &str) -> Result<Quote> {
        let conn = self.lock();

        let quote = conn
            .query_row(
                "SELECT id, rfq_id FROM marketplace_quotes WHERE id = ?1",
                params![quote_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|_| Error::Session("Quote not found".into()))?;
        let rfq_id = quote.1;

        // Generate contract ID
        let contract_id = format!("contract_{}", ulid::Ulid::new());

        // Mark quote accepted
        conn.execute(
            "UPDATE marketplace_quotes SET accepted = 1, contract_id = ?1 WHERE id = ?2",
            params![contract_id, quote_id],
        )?;

        // Mark RFQ as accepted
        conn.execute(
            "UPDATE marketplace_rfqs SET status = 'accepted', updated_at = datetime('now') WHERE id = ?1",
            params![rfq_id],
        )?;
        drop(conn);

        let mut quote = self.get_quote(quote_id)?;
        quote.accepted = true;
        quote.contract_id = Some(contract_id);
        Ok(quote)
    }

    /// Mark contract as created on-chain.
    pub fn mark_contracted(&self, rfq_id: &str) -> Result<()> {
        self.update_rfq_status(rfq_id, RfqStatus::Contracted)
    }

    // ── Stats ─────────────────────────────────────────────────────────

    pub fn get_stats(&self) -> Result<MarketplaceStats> {
        let conn = self.lock();
        let open_rfqs: usize = conn.query_row(
            "SELECT COUNT(*) FROM marketplace_rfqs WHERE status IN ('open', 'quoted')",
            [],
            |row| row.get(0),
        )?;
        let total_rfqs: usize =
            conn.query_row("SELECT COUNT(*) FROM marketplace_rfqs", [], |row| {
                row.get(0)
            })?;
        let total_quotes: usize =
            conn.query_row("SELECT COUNT(*) FROM marketplace_quotes", [], |row| {
                row.get(0)
            })?;
        let contracted: usize = conn.query_row(
            "SELECT COUNT(*) FROM marketplace_rfqs WHERE status = 'contracted'",
            [],
            |row| row.get(0),
        )?;
        let open_value: i64 = conn.query_row(
            "SELECT COALESCE(SUM(budget_cents), 0) FROM marketplace_rfqs WHERE status IN ('open', 'quoted')",
            [],
            |row| row.get(0),
        )?;

        Ok(MarketplaceStats {
            open_rfqs,
            total_rfqs,
            total_quotes,
            contracted,
            open_value_cents: open_value as u64,
        })
    }
}

// ── Row parser helper ─────────────────────────────────────────────────────

fn parse_rfq_row(row: &rusqlite::Row<'_>) -> std::result::Result<Rfq, rusqlite::Error> {
    let caps_str: String = row.get(6)?;
    let sla_str: String = row.get(9)?;
    Ok(Rfq {
        id: row.get(0)?,
        creator: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        budget_cents: row.get::<_, i64>(4)? as u64,
        deadline: row.get(5)?,
        required_capabilities: serde_json::from_str(&caps_str).unwrap_or_default(),
        required_trust_model: row.get(7)?,
        required_sandbox_config_hash: row.get(8)?,
        sla_metrics: serde_json::from_str(&sla_str).unwrap_or_default(),
        status: RfqStatus::from_str(&row.get::<_, String>(10)?),
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> MarketplaceStore {
        MarketplaceStore::open_in_memory().unwrap()
    }

    fn sample_sla() -> Vec<SlaRequirement> {
        vec![SlaRequirement {
            name: "sandbox".into(),
            target: "landlock+seccomp".into(),
            require_landlock: true,
            require_seccomp: true,
        }]
    }

    #[test]
    fn test_create_rfq() {
        let store = test_store();
        let rfq = store
            .create_rfq(
                "agent-alice",
                "Execute trading strategy backtest",
                "Backtest a mean-reversion strategy on SOL with 1Y data",
                5000, // $50.00
                "2026-07-01T00:00:00Z",
                &["trading".into(), "backtest".into(), "python".into()],
                Some("KernelSandbox"),
                Some("sha256:abc123"),
                &sample_sla(),
            )
            .unwrap();

        assert_eq!(rfq.creator, "agent-alice");
        assert_eq!(rfq.budget_cents, 5000);
        assert_eq!(rfq.status, RfqStatus::Open);
        assert_eq!(rfq.required_capabilities.len(), 3);
        assert_eq!(rfq.required_trust_model.as_deref(), Some("KernelSandbox"));
    }

    #[test]
    fn test_full_rfq_to_contract_flow() {
        let store = test_store();

        // 1. Create RFQ
        let rfq = store
            .create_rfq(
                "alice",
                "Task",
                "Desc",
                1000,
                "2026-12-31",
                &["compute".into()],
                None,
                None,
                &[],
            )
            .unwrap();
        assert_eq!(rfq.status, RfqStatus::Open);

        // 2. Agent submits quote
        let quote = store
            .submit_quote(
                &rfq.id,
                "agent-bob",
                800,
                3600,
                "Match: compute capability, KernelSandbox trust",
                "KernelSandbox",
                Some("sha256:def456"),
            )
            .unwrap();
        assert_eq!(quote.price_cents, 800);
        assert_eq!(quote.agent_id, "agent-bob");

        // RFQ should now be quoted
        let rfq = store.get_rfq(&rfq.id).unwrap();
        assert_eq!(rfq.status, RfqStatus::Quoted);

        // 3. Accept quote
        let accepted = store.accept_quote(&quote.id).unwrap();
        assert!(accepted.accepted);
        assert!(accepted.contract_id.is_some());

        // RFQ should now be accepted
        let rfq = store.get_rfq(&rfq.id).unwrap();
        assert_eq!(rfq.status, RfqStatus::Accepted);

        // 4. Mark contracted
        store.mark_contracted(&rfq.id).unwrap();
        let rfq = store.get_rfq(&rfq.id).unwrap();
        assert_eq!(rfq.status, RfqStatus::Contracted);
    }

    #[test]
    fn test_quote_on_closed_rfq_fails() {
        let store = test_store();
        let rfq = store
            .create_rfq("alice", "Task", "", 100, "2026-12-31", &[], None, None, &[])
            .unwrap();

        // Accept a quote first (closes the RFQ)
        let quote = store
            .submit_quote(&rfq.id, "bob", 50, 60, "ok", "SelfAttested", None)
            .unwrap();
        store.accept_quote(&quote.id).unwrap();

        // Now submit another quote — should fail
        let result = store.submit_quote(&rfq.id, "charlie", 40, 30, "ok", "SelfAttested", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not open"));
    }

    #[test]
    fn test_multiple_quotes_lowest_price_first() {
        let store = test_store();
        let rfq = store
            .create_rfq(
                "alice",
                "Task",
                "",
                1000,
                "2026-12-31",
                &[],
                None,
                None,
                &[],
            )
            .unwrap();

        store
            .submit_quote(&rfq.id, "agent-1", 500, 100, "", "", None)
            .unwrap();
        store
            .submit_quote(&rfq.id, "agent-2", 200, 200, "", "", None)
            .unwrap();
        store
            .submit_quote(&rfq.id, "agent-3", 800, 50, "", "", None)
            .unwrap();

        let quotes = store.list_quotes_for_rfq(&rfq.id).unwrap();
        assert_eq!(quotes.len(), 3);
        // Sorted by price ascending
        assert_eq!(quotes[0].price_cents, 200);
        assert_eq!(quotes[1].price_cents, 500);
        assert_eq!(quotes[2].price_cents, 800);
    }

    #[test]
    fn test_marketplace_stats() {
        let store = test_store();

        let rfq1 = store
            .create_rfq("a", "T1", "", 100, "2026-12-31", &[], None, None, &[])
            .unwrap();
        let rfq2 = store
            .create_rfq("a", "T2", "", 300, "2026-12-31", &[], None, None, &[])
            .unwrap();

        store
            .submit_quote(&rfq1.id, "b", 50, 60, "", "", None)
            .unwrap();

        let stats = store.get_stats().unwrap();
        assert_eq!(stats.open_rfqs, 2);
        assert_eq!(stats.total_rfqs, 2);
        assert_eq!(stats.total_quotes, 1);
        assert_eq!(stats.open_value_cents, 400); // 100 + 300

        // Close one RFQ
        let quote = store
            .submit_quote(&rfq2.id, "b", 200, 60, "", "", None)
            .unwrap();
        store.accept_quote(&quote.id).unwrap();

        let stats = store.get_stats().unwrap();
        assert_eq!(stats.open_rfqs, 1); // only rfq1 still open
        assert_eq!(stats.open_value_cents, 100);
    }

    #[test]
    fn test_list_rfqs_by_status() {
        let store = test_store();
        store
            .create_rfq("a", "Open RFQ", "", 100, "2026-12-31", &[], None, None, &[])
            .unwrap();
        let rfq2 = store
            .create_rfq(
                "a",
                "Cancelled RFQ",
                "",
                200,
                "2026-12-31",
                &[],
                None,
                None,
                &[],
            )
            .unwrap();
        store.cancel_rfq(&rfq2.id).unwrap();

        let open = store.list_rfqs(Some("open")).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].title, "Open RFQ");

        let cancelled = store.list_rfqs(Some("cancelled")).unwrap();
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].title, "Cancelled RFQ");
    }

    #[test]
    fn test_rfq_status_lifecycle() {
        let store = test_store();
        let rfq = store
            .create_rfq("a", "T", "", 100, "2026-12-31", &[], None, None, &[])
            .unwrap();
        assert_eq!(rfq.status, RfqStatus::Open);

        store
            .update_rfq_status(&rfq.id, RfqStatus::Expired)
            .unwrap();
        let rfq = store.get_rfq(&rfq.id).unwrap();
        assert_eq!(rfq.status, RfqStatus::Expired);
    }
}
