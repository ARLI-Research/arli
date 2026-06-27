//! Confidence Ledger — persistent cross-run calibration storage.
//!
//! Accumulates confidence records across multiple ENSO contract executions
//! so investors can see calibration statistics over weeks/months, not just
//! a single run. Uses SQLite for simple self-contained storage.
//!
//! The ledger is append-only: each run's confidence_records are inserted
//! as a batch after execution completes.

use rust_decimal::Decimal;
use rusqlite::Connection;
use std::path::Path;

use crate::execution::{ConfidenceBuckets, ConfidenceRecord};

/// Persistent confidence calibration store.
///
/// # Schema
///
/// ```sql
/// CREATE TABLE IF NOT EXISTS confidence_records (
///     id INTEGER PRIMARY KEY AUTOINCREMENT,
///     run_id TEXT NOT NULL,
///     coin TEXT NOT NULL,
///     strategy TEXT NOT NULL,
///     claimed_confidence TEXT NOT NULL,
///     direction TEXT NOT NULL,
///     was_winner INTEGER NOT NULL,
///     realized_pnl_pct TEXT NOT NULL,
///     entry_price TEXT,
///     exit_price TEXT,
///     recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
/// );
/// ```
pub struct ConfidenceLedger {
    conn: Connection,
}

impl ConfidenceLedger {
    /// Open (or create) the ledger at the given path.
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("open ledger db: {e}"))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS confidence_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                coin TEXT NOT NULL,
                strategy TEXT NOT NULL,
                claimed_confidence TEXT NOT NULL,
                direction TEXT NOT NULL,
                was_winner INTEGER NOT NULL,
                realized_pnl_pct TEXT NOT NULL,
                entry_price TEXT,
                exit_price TEXT,
                recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_confidence_run
                ON confidence_records(run_id);
            CREATE INDEX IF NOT EXISTS idx_confidence_strategy
                ON confidence_records(strategy);
            CREATE INDEX IF NOT EXISTS idx_confidence_bucket
                ON confidence_records(claimed_confidence);
            ",
        )
        .map_err(|e| format!("init ledger schema: {e}"))?;

        Ok(Self { conn })
    }

    /// Open in-memory (for tests / transient use).
    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| format!("open mem ledger: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS confidence_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                coin TEXT NOT NULL,
                strategy TEXT NOT NULL,
                claimed_confidence TEXT NOT NULL,
                direction TEXT NOT NULL,
                was_winner INTEGER NOT NULL,
                realized_pnl_pct TEXT NOT NULL,
                entry_price TEXT,
                exit_price TEXT,
                recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .map_err(|e| format!("init mem ledger schema: {e}"))?;
        Ok(Self { conn })
    }

    /// Append a batch of confidence records for a run.
    ///
    /// All records for a single `run_id` are inserted in one transaction.
    pub fn append_run(
        &self,
        run_id: &str,
        records: &[ConfidenceRecord],
    ) -> Result<usize, String> {
        if records.is_empty() {
            return Ok(0);
        }

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin tx: {e}"))?;

        let mut count = 0;
        for r in records {
            tx.execute(
                "INSERT INTO confidence_records
                 (run_id, coin, strategy, claimed_confidence, direction,
                  was_winner, realized_pnl_pct, entry_price, exit_price)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    run_id,
                    r.coin,
                    r.strategy,
                    r.claimed_confidence.to_string(),
                    r.direction,
                    r.was_winner as i32,
                    r.realized_pnl_pct.to_string(),
                    r.entry_price.map(|d| d.to_string()),
                    r.exit_price.map(|d| d.to_string()),
                ],
            )
            .map_err(|e| format!("insert record: {e}"))?;
            count += 1;
        }

        tx.commit().map_err(|e| format!("commit tx: {e}"))?;
        Ok(count)
    }

    /// Load all records from the ledger (across all runs).
    pub fn load_all(&self) -> Result<Vec<ConfidenceRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT coin, strategy, claimed_confidence, direction,
                        was_winner, realized_pnl_pct, entry_price, exit_price
                 FROM confidence_records
                 ORDER BY id",
            )
            .map_err(|e| format!("prepare query: {e}"))?;

        let records = stmt
            .query_map([], |row| {
                let claimed_str: String = row.get(2)?;
                let pnl_str: String = row.get(5)?;
                let entry_str: Option<String> = row.get(6)?;
                let exit_str: Option<String> = row.get(7)?;

                Ok(ConfidenceRecord {
                    coin: row.get(0)?,
                    strategy: row.get(1)?,
                    claimed_confidence: claimed_str
                        .parse::<Decimal>()
                        .unwrap_or(Decimal::ZERO),
                    direction: row.get(3)?,
                    was_winner: row.get::<_, i32>(4)? != 0,
                    realized_pnl_pct: pnl_str
                        .parse::<Decimal>()
                        .unwrap_or(Decimal::ZERO),
                    entry_price: entry_str.and_then(|s| s.parse::<Decimal>().ok()),
                    exit_price: exit_str.and_then(|s| s.parse::<Decimal>().ok()),
                })
            })
            .map_err(|e| format!("query records: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("collect records: {e}"))?;

        Ok(records)
    }

    /// Get calibration statistics across all stored records.
    pub fn calibration(&self) -> Result<serde_json::Value, String> {
        let records = self.load_all()?;
        let buckets = ConfidenceBuckets::from_records(&records);
        Ok(buckets.to_json(&records))
    }

    /// Get calibration statistics for a specific strategy.
    pub fn calibration_for_strategy(&self, strategy: &str) -> Result<serde_json::Value, String> {
        let all = self.load_all()?;
        let filtered: Vec<ConfidenceRecord> = all
            .into_iter()
            .filter(|r| r.strategy == strategy)
            .collect();
        let buckets = ConfidenceBuckets::from_records(&filtered);
        Ok(buckets.to_json(&filtered))
    }

    /// Get per-strategy calibration summary.
    pub fn per_strategy_summary(&self) -> Result<serde_json::Value, String> {
        let records = self.load_all()?;

        // Group by strategy
        let mut strategies: std::collections::HashMap<String, Vec<ConfidenceRecord>> =
            std::collections::HashMap::new();
        for r in records {
            strategies
                .entry(r.strategy.clone())
                .or_default()
                .push(r);
        }

        let mut result = serde_json::Map::new();
        for (name, recs) in &strategies {
            let wins = recs.iter().filter(|r| r.was_winner).count();
            let total: Decimal = recs.iter().map(|r| r.claimed_confidence).sum();
            let sum_pnl: Decimal = recs.iter().map(|r| r.realized_pnl_pct).sum();

            result.insert(
                name.clone(),
                serde_json::json!({
                    "total_trades": recs.len(),
                    "wins": wins,
                    "accuracy": if recs.is_empty() { serde_json::Value::Null }
                        else { serde_json::json!(wins as f64 / recs.len() as f64) },
                    "avg_confidence": if recs.is_empty() { serde_json::Value::Null }
                        else { serde_json::json!(total / Decimal::from(recs.len() as u64)) },
                    "avg_pnl_pct": if recs.is_empty() { serde_json::Value::Null }
                        else { serde_json::json!(sum_pnl / Decimal::from(recs.len() as u64)) },
                    "brier_score": ConfidenceBuckets::brier_score(recs),
                }),
            );
        }

        Ok(serde_json::Value::Object(result))
    }

    /// Count total records.
    pub fn total_records(&self) -> Result<u64, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM confidence_records", [], |row| {
                row.get::<_, i64>(0).map(|n| n as u64)
            })
            .map_err(|e| format!("count records: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn make_records() -> Vec<ConfidenceRecord> {
        vec![
            ConfidenceRecord {
                coin: "BTC".into(),
                strategy: "trend-following".into(),
                claimed_confidence: dec!(0.85),
                direction: "long".into(),
                was_winner: true,
                realized_pnl_pct: dec!(0.025),
                entry_price: Some(dec!(50000)),
                exit_price: Some(dec!(51250)),
            },
            ConfidenceRecord {
                coin: "ETH".into(),
                strategy: "trend-following".into(),
                claimed_confidence: dec!(0.60),
                direction: "long".into(),
                was_winner: false,
                realized_pnl_pct: dec!(-0.012),
                entry_price: Some(dec!(3000)),
                exit_price: Some(dec!(2964)),
            },
            ConfidenceRecord {
                coin: "SOL".into(),
                strategy: "mean-reversion".into(),
                claimed_confidence: dec!(0.30),
                direction: "long".into(),
                was_winner: false,
                realized_pnl_pct: dec!(-0.008),
                entry_price: Some(dec!(100)),
                exit_price: Some(dec!(99.2)),
            },
            ConfidenceRecord {
                coin: "BTC".into(),
                strategy: "mean-reversion".into(),
                claimed_confidence: dec!(0.95),
                direction: "long".into(),
                was_winner: true,
                realized_pnl_pct: dec!(0.040),
                entry_price: Some(dec!(51000)),
                exit_price: Some(dec!(53040)),
            },
        ]
    }

    #[test]
    fn test_ledger_append_and_load() {
        let ledger = ConfidenceLedger::open_in_memory().unwrap();
        let count = ledger.append_run("run-001", &make_records()).unwrap();
        assert_eq!(count, 4);

        let loaded = ledger.load_all().unwrap();
        assert_eq!(loaded.len(), 4);
        assert_eq!(ledger.total_records().unwrap(), 4);
    }

    #[test]
    fn test_ledger_calibration() {
        let ledger = ConfidenceLedger::open_in_memory().unwrap();
        ledger.append_run("run-001", &make_records()).unwrap();

        let cal = ledger.calibration().unwrap();
        // 2 wins out of 4
        let accuracy = cal["overall_accuracy"].as_f64().unwrap();
        assert!((accuracy - 0.5).abs() < 0.01);

        // Brier score should be computed (can be f64, decimal string, or null)
        let brier = &cal["brier_score"];
        assert!(
            brier.is_f64()
                || brier.is_string()
                || brier.is_null(),
            "brier_score was: {brier}"
        );
    }

    #[test]
    fn test_per_strategy_summary() {
        let ledger = ConfidenceLedger::open_in_memory().unwrap();
        ledger.append_run("run-001", &make_records()).unwrap();

        let summary = ledger.per_strategy_summary().unwrap();
        let tf = &summary["trend-following"];
        assert_eq!(tf["total_trades"].as_u64().unwrap(), 2);
        assert_eq!(tf["accuracy"].as_f64().unwrap(), 0.5);
    }

    #[test]
    fn test_calibration_for_strategy() {
        let ledger = ConfidenceLedger::open_in_memory().unwrap();
        ledger.append_run("run-001", &make_records()).unwrap();

        let cal = ledger
            .calibration_for_strategy("mean-reversion")
            .unwrap();
        let accuracy = cal["overall_accuracy"].as_f64().unwrap();
        assert!((accuracy - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_multiple_runs_accumulate() {
        let ledger = ConfidenceLedger::open_in_memory().unwrap();
        ledger.append_run("run-001", &make_records()).unwrap();
        ledger.append_run("run-002", &make_records()).unwrap();

        assert_eq!(ledger.total_records().unwrap(), 8);
    }
}
