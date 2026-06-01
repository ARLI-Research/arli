//! Persistent cross-session memory store.
//!
//! Three targets:
//! - `user`: who the user is — preferences, name, role, communication style
//! - `memory`: agent notes — environment facts, project conventions, lessons learned
//! - `correction`: feedback loop — user corrections the agent learns from
//!
//! Two recall modes:
//! - `recall` — fast FTS5 keyword search
//! - `reflect` — deep LLM-powered synthesis across all memories for a target
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
                target TEXT NOT NULL CHECK(target IN ('user', 'memory', 'correction')),
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content,
                target UNINDEXED,
                content_rowid='id',
                content='memories'
            );",
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
            END;",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open an in-memory store (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target TEXT NOT NULL CHECK(target IN ('user', 'memory', 'correction')),
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
            END;",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Add a new memory entry. Returns the new ID.
    pub fn add(&self, target: &str, content: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO memories (target, content) VALUES (?1, ?2)",
            params![target, content],
        )?;
        let id = conn.last_insert_rowid();
        tracing::info!(
            "Memory added: [{target}] id={id} ({:.80}...)",
            &content[..content.len().min(80)]
        );
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

    /// Reflect: deep LLM-powered synthesis across all memories for a target.
    ///
    /// Returns a prompt string that the calling agent can send to its LLM
    /// for synthesis — finding connections, identifying patterns, and
    /// answering the query using only the stored memories.
    pub fn reflect(&self, target: &str, query: &str) -> Result<String> {
        let memories = self.get_all(Some(target))?;

        let mut prompt = format!(
            "Synthesize memories about '{}' in response to query '{}'.\n\nMemories:\n",
            target, query
        );
        for m in &memories {
            prompt.push_str(&format!("- [{}] {}\n", m.created_at, m.content));
        }
        prompt.push_str(
            "\nFind connections, identify patterns, and answer the query using ONLY the memories above.",
        );
        Ok(prompt)
    }

    /// Search memories using FTS5 — also aliased as "recall".
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.target, m.content, m.created_at, m.updated_at
             FROM memories_fts f
             JOIN memories m ON f.rowid = m.id
             WHERE memories_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![query, limit as i64], Self::map_row)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// Add a user correction to the feedback loop.
    ///
    /// Stores both the original (what the agent did/said) and the
    /// correction (what the user wanted instead) in a single entry
    /// with target="correction".
    pub fn add_correction(&self, original: &str, correction: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let content = format!("USER: {}\nCORRECTED: {}", original, correction);
        conn.execute(
            "INSERT INTO memories (target, content) VALUES ('correction', ?1)",
            params![content],
        )?;
        let id = conn.last_insert_rowid();
        tracing::info!(
            "Correction saved: id={id} ({:.80}... → {:.80}...)",
            &original[..original.len().min(80)],
            &correction[..correction.len().min(80)]
        );
        Ok(id)
    }

    /// Retrieve all stored corrections (target="correction").
    pub fn get_corrections(&self) -> Result<Vec<MemoryEntry>> {
        self.get_all(Some("correction"))
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
        let updated = store
            .replace(
                "memory",
                "Project uses cargo",
                "Project uses cargo workspace",
            )
            .unwrap();
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

    #[test]
    fn test_reflect() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.add("memory", "Project uses Rust").unwrap();
        store.add("memory", "Tests are written with cargo test").unwrap();
        store.add("memory", "CI runs on GitHub Actions").unwrap();

        let prompt = store.reflect("memory", "What tools do we use?").unwrap();
        assert!(prompt.contains("Synthesize memories about 'memory'"));
        assert!(prompt.contains("Project uses Rust"));
        assert!(prompt.contains("cargo test"));
        assert!(prompt.contains("GitHub Actions"));
        assert!(prompt.contains("ONLY the memories above"));
    }

    #[test]
    fn test_reflect_empty() {
        let store = MemoryStore::open_in_memory().unwrap();
        let prompt = store.reflect("user", "any query").unwrap();
        assert!(prompt.contains("Synthesize memories about 'user'"));
        assert!(prompt.contains("Find connections"));
    }

    #[test]
    fn test_add_and_get_corrections() {
        let store = MemoryStore::open_in_memory().unwrap();
        let id = store.add_correction("I used python", "Use uv instead").unwrap();
        assert!(id > 0);

        let corrections = store.get_corrections().unwrap();
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].target, "correction");
        assert_eq!(
            corrections[0].content,
            "USER: I used python\nCORRECTED: Use uv instead"
        );
    }

    #[test]
    fn test_multiple_corrections() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.add_correction("bad output 1", "correct output 1").unwrap();
        store.add_correction("bad output 2", "correct output 2").unwrap();

        let corrections = store.get_corrections().unwrap();
        assert_eq!(corrections.len(), 2);
    }
}

/// appropriate backend based on the MemoryConfig.
///
/// Supported providers:
/// - "builtin" / default — SQLite local store
/// - "mem0" — Cloud-hosted memory API (mem0.ai, needs API key)
/// - "chroma" — Local ChromaDB vector database
/// - "qdrant" — Local/cloud Qdrant vector database
/// - "byterover" — Cloud-hosted memory (bytabox.ai, needs API key)
/// - "hindsight" — Local/cloud memory store (API key or local)
/// - "holographic" — Local holographic memory store
/// - "honcho" — Local/cloud memory for AI agents (API key or local)
/// - "openviking" — API key / local memory store
/// - "retaindb" — API key / local vector memory
/// - "supermemory" — Cloud memory API (supermemory.ai, needs API key)
/// - "agentmemory" — API key / local memory for agents
///
/// For external providers, this returns the existing SQLite MemoryStore
/// and logs the configured provider. Full integration stubs are provided
/// for future implementation.
pub fn create_memory_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    match config.provider.as_str() {
        "mem0" => create_mem0_store(config, db_path),
        "chroma" => create_chroma_store(config, db_path),
        "qdrant" => create_qdrant_store(config, db_path),
        "byterover" => create_byterover_store(config, db_path),
        "hindsight" => create_hindsight_store(config, db_path),
        "holographic" => create_holographic_store(config, db_path),
        "honcho" => create_honcho_store(config, db_path),
        "openviking" => create_openviking_store(config, db_path),
        "retaindb" => create_retaindb_store(config, db_path),
        "supermemory" => create_supermemory_store(config, db_path),
        "agentmemory" => create_agentmemory_store(config, db_path),
        _ => {
            // "builtin" or unknown — use SQLite local store
            tracing::info!("Memory provider: builtin (SQLite)");
            MemoryStore::open(db_path)
        }
    }
}

/// Stub: Mem0 memory backend.
///
/// Mem0 is a cloud-hosted memory API at https://api.mem0.ai/v1.
/// This stub logs that mem0 is configured and falls back to SQLite.
/// Full integration would use an HTTP client to the Mem0 API.
fn create_mem0_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    let base_url = if config.base_url.is_empty() {
        "https://api.mem0.ai/v1"
    } else {
        &config.base_url
    };
    tracing::info!(
        "Memory provider: mem0 (API: {}, key: {})",
        base_url,
        if config.api_key.is_empty() {
            "not set"
        } else {
            "configured"
        }
    );
    // TODO: Implement full mem0 HTTP client integration
    // For now, fall back to SQLite for backward compatibility
    MemoryStore::open(db_path)
}

/// Stub: ChromaDB memory backend.
///
/// ChromaDB is a local open-source vector database.
/// Default URL: http://localhost:8000 (env CHROMA_URL).
/// This stub logs that chroma is configured and falls back to SQLite.
fn create_chroma_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    let base_url = if config.base_url.is_empty() {
        "http://localhost:8000"
    } else {
        &config.base_url
    };
    tracing::info!("Memory provider: chroma (URL: {})", base_url);
    // TODO: Implement full ChromaDB HTTP client integration
    MemoryStore::open(db_path)
}

/// Stub: Qdrant memory backend.
///
/// Qdrant is a vector database (local or cloud).
/// Requires QDRANT_URL and QDRANT_API_KEY env vars.
/// This stub logs that qdrant is configured and falls back to SQLite.
fn create_qdrant_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    let base_url = if config.base_url.is_empty() {
        "http://localhost:6333"
    } else {
        &config.base_url
    };
    tracing::info!(
        "Memory provider: qdrant (URL: {}, key: {})",
        base_url,
        if config.api_key.is_empty() {
            "not set"
        } else {
            "configured"
        }
    );
    // TODO: Implement full Qdrant client integration
    MemoryStore::open(db_path)
}

// ── New memory provider stubs ──

/// Byterover — cloud-hosted memory (bytabox.ai, requires API key).
fn create_byterover_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    tracing::info!(
        "Memory provider: byterover (key: {})",
        if config.api_key.is_empty() {
            "not set"
        } else {
            "configured"
        }
    );
    MemoryStore::open(db_path)
}

/// Hindsight — local/cloud memory store (API key or local).
fn create_hindsight_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    tracing::info!(
        "Memory provider: hindsight (mode: {})",
        if config.api_key.is_empty() {
            "local"
        } else {
            "cloud"
        }
    );
    MemoryStore::open(db_path)
}

/// Holographic — local holographic memory store.
fn create_holographic_store(
    _config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    tracing::info!("Memory provider: holographic (local)");
    MemoryStore::open(db_path)
}

/// Honcho — local/cloud memory for AI agents.
fn create_honcho_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    tracing::info!(
        "Memory provider: honcho (mode: {})",
        if config.api_key.is_empty() {
            "local"
        } else {
            "cloud"
        }
    );
    MemoryStore::open(db_path)
}

/// OpenViking — API key / local memory store.
fn create_openviking_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    tracing::info!(
        "Memory provider: openviking (mode: {})",
        if config.api_key.is_empty() {
            "local"
        } else {
            "cloud"
        }
    );
    MemoryStore::open(db_path)
}

/// RetainDB — API key / local vector memory.
fn create_retaindb_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    tracing::info!(
        "Memory provider: retaindb (mode: {})",
        if config.api_key.is_empty() {
            "local"
        } else {
            "cloud"
        }
    );
    MemoryStore::open(db_path)
}

/// Supermemory — cloud memory API (supermemory.ai, requires API key).
fn create_supermemory_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    tracing::info!(
        "Memory provider: supermemory (key: {})",
        if config.api_key.is_empty() {
            "not set"
        } else {
            "configured"
        }
    );
    MemoryStore::open(db_path)
}

/// AgentMemory — API key / local memory for agents.
fn create_agentmemory_store(
    config: &crate::config::MemoryConfig,
    db_path: PathBuf,
) -> Result<MemoryStore> {
    tracing::info!(
        "Memory provider: agentmemory (mode: {})",
        if config.api_key.is_empty() {
            "local"
        } else {
            "cloud"
        }
    );
    MemoryStore::open(db_path)
}
