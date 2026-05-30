use rusqlite::{Connection, params};
use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::providers::{ChatMessage, Role};

/// SQLite-backed session store with message persistence.
///
/// Similar to Hermes' hermes_state.py — stores full conversation history
/// for resume, search, and compression.
pub struct SessionStore {
    conn: Connection,
}

impl SessionStore {
    /// Open or create a session database at the given path.
    pub fn open(path: PathBuf) -> Result<Self> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Session(format!("Cannot create DB directory: {}", e))
            })?;
        }

        let conn = Connection::open(&path)?;

        // Enable WAL mode for concurrent reads
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Open an in-memory session store (for tests)
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                name TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                parent_session_id TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                compaction_count INTEGER NOT NULL DEFAULT 0,
                metadata TEXT DEFAULT '{}'
            );

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT,
                tool_calls TEXT,
                tool_call_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS compactions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                compacted_at TEXT NOT NULL DEFAULT (datetime('now')),
                summary TEXT NOT NULL,
                tokens_before INTEGER,
                tokens_after INTEGER,
                compacted_count INTEGER,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session
                ON messages(session_id, id);

            -- FTS5 virtual table for full-text search
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                content,
                role,
                session_id UNINDEXED,
                content_rowid='id',
                content='messages'
            );"
        )?;

        // Create triggers to keep FTS in sync
        self.conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, content, role, session_id)
                VALUES (new.id, new.content, new.role, new.session_id);
            END;

            CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, content, role, session_id)
                VALUES ('delete', old.id, old.content, old.role, old.session_id);
            END;

            CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, content, role, session_id)
                VALUES ('delete', old.id, old.content, old.role, old.session_id);
                INSERT INTO messages_fts(rowid, content, role, session_id)
                VALUES (new.id, new.content, new.role, new.session_id);
            END;"
        )?;

        Ok(())
    }

    /// Create a new session, returning its ID
    pub fn create_session(&self, name: Option<&str>) -> Result<String> {
        let id = ulid::Ulid::new().to_string();
        let name = name.unwrap_or("untitled");

        self.conn.execute(
            "INSERT INTO sessions (id, name) VALUES (?1, ?2)",
            params![id, name],
        )?;

        tracing::info!("Created session: {} ({})", id, name);
        Ok(id)
    }

    /// Save a message to the session
    pub fn save_message(&self, session_id: &str, message: &ChatMessage) -> Result<()> {
        let role = match message.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };

        let tool_calls_json = message
            .tool_calls
            .as_ref()
            .map(|tc| serde_json::to_string(tc))
            .transpose()?;

        self.conn.execute(
            "INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                role,
                message.content,
                tool_calls_json,
                message.tool_call_id
            ],
        )?;

        // Update session timestamp
        self.conn.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )?;

        Ok(())
    }

    /// Load all messages for a session in chronological order
    pub fn load_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, tool_calls, tool_call_id
             FROM messages
             WHERE session_id = ?1
             ORDER BY id ASC"
        )?;

        let messages = stmt
            .query_map(params![session_id], |row| {
                let role_str: String = row.get(0)?;
                let content: Option<String> = row.get(1)?;
                let tool_calls_json: Option<String> = row.get(2)?;
                let tool_call_id: Option<String> = row.get(3)?;

                let role = match role_str.as_str() {
                    "system" => Role::System,
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    "tool" => Role::Tool,
                    _ => Role::User,
                };

                let tool_calls: Option<Vec<crate::providers::ToolCall>> = tool_calls_json
                    .and_then(|json| serde_json::from_str(&json).ok());

                Ok(ChatMessage {
                    role,
                    content,
                    tool_calls,
                    tool_call_id,
                    name: None,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(messages)
    }

    /// Count messages in a session
    pub fn message_count(&self, session_id: &str) -> Result<usize> {
        let count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;

        Ok(count)
    }

    /// List recent sessions
    pub fn list_sessions(&self, limit: usize) -> Result<Vec<SessionInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, created_at, updated_at, status
             FROM sessions
             ORDER BY updated_at DESC
             LIMIT ?1"
        )?;

        let sessions = stmt
            .query_map(params![limit as i64], |row| {
                Ok(SessionInfo {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    status: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(sessions)
    }
    /// Full-text search across all sessions using FTS5.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        // Build SQL and params based on whether we filter by session
        let (sql, extra_param) = if let Some(sid) = session_id {
            (
                "SELECT m.content, m.role, m.session_id, m.id
                 FROM messages_fts f
                 JOIN messages m ON f.rowid = m.id
                 WHERE messages_fts MATCH ?1 AND m.session_id = ?2
                 ORDER BY rank
                 LIMIT ?3",
                Some(sid.to_string()),
            )
        } else {
            (
                "SELECT m.content, m.role, m.session_id, m.id
                 FROM messages_fts f
                 JOIN messages m ON f.rowid = m.id
                 WHERE messages_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
                None,
            )
        };

        let mut stmt = self.conn.prepare(sql)?;

        let results: Vec<SearchResult> = if let Some(ref sid) = extra_param {
            stmt.query_map(
                rusqlite::params![query, sid.as_str(), limit as i64],
                |row| {
                    Ok(SearchResult {
                        content: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        role: row.get::<_, String>(1)?,
                        session_id: row.get::<_, String>(2)?,
                        context_before: None,
                        context_after: None,
                    })
                },
            )?
            .filter_map(|r| r.ok())
            .take(limit)
            .collect()
        } else {
            stmt.query_map(
                rusqlite::params![query, limit as i64],
                |row| {
                    Ok(SearchResult {
                        content: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        role: row.get::<_, String>(1)?,
                        session_id: row.get::<_, String>(2)?,
                        context_before: None,
                        context_after: None,
                    })
                },
            )?
            .filter_map(|r| r.ok())
            .take(limit)
            .collect()
        };

        Ok(results)
    }
    /// Record a compaction event (for session lineage).
    pub fn record_compaction(
        &self,
        session_id: &str,
        summary: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO compactions (session_id, summary) VALUES (?1, ?2)",
            params![session_id, summary],
        )?;

        self.conn.execute(
            "UPDATE sessions SET compaction_count = compaction_count + 1, updated_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )?;

        tracing::info!("Recorded compaction for session {}", session_id);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub content: String,
    pub role: String,
    pub session_id: String,
    pub context_before: Option<String>,
    pub context_after: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
}
