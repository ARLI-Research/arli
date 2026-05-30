//! Persistent cross-session memory store.
//!
//! Two targets (like Hermes):
//! - `user`: who the user is — preferences, name, role, communication style
//! - `memory`: agent notes — environment facts, project conventions, lessons learned
//!
//! Stored in SQLite for durability across agent restarts.

use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{Error, Result};

/// A stored memory entry.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: i64,
    pub target: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Persistent memory store backed by SQLite.
pub struct MemoryStore {
    conn: Mutex<Connection>,
}

impl MemoryStore {
    /// Open a memory store at the given path, creating it if needed.
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Session(format!("Cannot create memory DB dir: {}", e)))?;
        }

        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        // Schema
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target TEXT NOT NULL CHECK(target IN ('user', 'memory')),
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS memories_fts USING fts5(
                content,
                target UNINDEXED,
                content_rowid='id',
                content='memories'
            );"
        )?;

        // FTS5 sync triggers
        conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, content, target)
                VALUES (new.id, new.content, new.target);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, content, target)
                VALUES ('delete', old.id, old.content, old.target);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, content, target)
                VALUES ('delete', old.id, old.content, old.target);
                INSERT INTO memories_fts(rowid, content, target)
                VALUES (new.id, new.content, new.target);
            END;"
        )?;

        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Open an in-memory store (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target TEXT NOT NULL CHECK(target IN ('user', 'memory')),
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content,
                target UNINDEXED,
                content_rowid='id',
                content='memories'
            );

            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, content, target)
                VALUES (new.id, new.content, new.target);
            END;"
        )?;

        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Add a new memory entry. Returns the new ID.
    pub fn add(&self, target: &str, content: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO memories (target, content) VALUES (?1, ?2)",
            params![target, content],
        )?;
        let id = conn.last_insert_rowid();
        tracing::info!("Memory added: [{target}] id={id} ({:.80}...)", &content[..content.len().min(80)]);
        Ok(id)
    }

    /// Replace a memory entry by exact content match.
    pub fn replace(&self, target: &str, old_text: &str, new_content: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let updated = conn.execute(
            "UPDATE memories SET content = ?1, updated_at = datetime('now') WHERE target = ?2 AND content = ?3",
            params![new_content, target, old_text],
        )?;
        tracing::info!("Memory replaced: [{target}] {updated} row(s)");
        Ok(updated)
    }

    /// Remove a memory entry by content match.
    pub fn remove(&self, target: &str, content_to_remove: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM memories WHERE target = ?1 AND content = ?2",
            params![target, content_to_remove],
        )?;
        tracing::info!("Memory removed: [{target}] {removed} row(s)");
        Ok(removed)
    }

    /// Get all memories, optionally filtered by target.
    pub fn get_all(&self, target: Option<&str>) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().unwrap();
        let (query, param) = if let Some(t) = target {
            (
                "SELECT id, target, content, created_at, updated_at FROM memories WHERE target = ?1 ORDER BY updated_at DESC",
                Some(t.to_string()),
            )
        } else {
            (
                "SELECT id, target, content, created_at, updated_at FROM memories ORDER BY updated_at DESC",
                None,
            )
        };

        let mut stmt = conn.prepare(query)?;
        let rows = if let Some(ref p) = param {
            stmt.query_map(params![p.as_str()], Self::map_row)?
        } else {
            stmt.query_map([], Self::map_row)?
        };

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// Count memories per target.
    pub fn count(&self) -> Result<(usize, usize)> {
        let conn = self.conn.lock().unwrap();
        let user_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE target = 'user'",
            [],
            |r| r.get(0),
        )?;
        let mem_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE target = 'memory'",
            [],
            |r| r.get(0),
        )?;
        Ok((user_count, mem_count))
    }

    /// Search memories using FTS5.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.target, m.content, m.created_at, m.updated_at
             FROM memories_fts f
             JOIN memories m ON f.rowid = m.id
             WHERE memories_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2"
        )?;

        let rows = stmt.query_map(params![query, limit as i64], Self::map_row)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    fn map_row(row: &rusqlite::Row<'_>) -> std::result::Result<MemoryEntry, rusqlite::Error> {
        Ok(MemoryEntry {
            id: row.get(0)?,
            target: row.get(1)?,
            content: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_retrieve() {
        let store = MemoryStore::open_in_memory().unwrap();
        let id = store.add("user", "Dmitry prefers Rust").unwrap();
        assert!(id > 0);

        let entries = store.get_all(Some("user")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "Dmitry prefers Rust");
    }

    #[test]
    fn test_replace_memory() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.add("memory", "Project uses cargo").unwrap();
        let updated = store.replace("memory", "Project uses cargo", "Project uses cargo workspace").unwrap();
        assert_eq!(updated, 1);

        let entries = store.get_all(Some("memory")).unwrap();
        assert_eq!(entries[0].content, "Project uses cargo workspace");
    }

    #[test]
    fn test_remove_memory() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.add("user", "Temporary note").unwrap();
        store.add("user", "Keep this one").unwrap();

        let removed = store.remove("user", "Temporary note").unwrap();
        assert_eq!(removed, 1);

        let entries = store.get_all(Some("user")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "Keep this one");
    }

    #[test]
    fn test_count_by_target() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.add("user", "User pref 1").unwrap();
        store.add("user", "User pref 2").unwrap();
        store.add("memory", "Env note 1").unwrap();

        let (user_count, mem_count) = store.count().unwrap();
        assert_eq!(user_count, 2);
        assert_eq!(mem_count, 1);
    }

    #[test]
    fn test_search() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.add("memory", "Rust toolchain: rustc 1.95.0").unwrap();
        store.add("memory", "Python version: 3.11").unwrap();

        let results = store.search("rustc", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("rustc"));
    }
}
