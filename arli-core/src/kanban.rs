//! Kanban task boards — persistent task tracking for agents.
//!
//! Agents use kanban boards to decompose work, track progress across
//! columns (backlog → todo → in_progress → review → done), and self-manage
//! their backlog. Each board is SQLite-backed with WAL mode for concurrent
//! access from multiple agent sessions.
//!
//! # Schema
//!
//! ```text
//! kanban_boards    — id, name, description, created_at, updated_at
//! kanban_columns   — id, board_id, name, position, wip_limit
//! kanban_cards     — id, column_id, title, description, priority, assignee, tags, due_date, ...
//! ```

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{Error, Result};

// ── Data model ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Critical,
    High,
    Medium,
    Low,
}

impl Priority {
    pub fn as_str(&self) -> &str {
        match self {
            Priority::Critical => "critical",
            Priority::High => "high",
            Priority::Medium => "medium",
            Priority::Low => "low",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "critical" => Priority::Critical,
            "high" => Priority::High,
            "medium" => Priority::Medium,
            _ => Priority::Low,
        }
    }

    /// Numeric weight for sorting (higher = more urgent).
    pub fn weight(&self) -> u8 {
        match self {
            Priority::Critical => 4,
            Priority::High => 3,
            Priority::Medium => 2,
            Priority::Low => 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanBoard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanColumn {
    pub id: String,
    pub board_id: String,
    pub name: String,
    pub position: usize,
    pub wip_limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanCard {
    pub id: String,
    pub column_id: String,
    pub board_id: String,
    pub title: String,
    pub description: String,
    pub priority: Priority,
    pub assignee: Option<String>,
    pub tags: Vec<String>,
    pub due_date: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardStats {
    pub board_id: String,
    pub board_name: String,
    pub total_cards: usize,
    pub columns: Vec<ColumnStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnStats {
    pub column_id: String,
    pub column_name: String,
    pub card_count: usize,
    pub wip_limit: Option<usize>,
    pub wip_breached: bool,
    pub cards: Vec<KanbanCard>,
}

// ── KanbanStore ────────────────────────────────────────────────────────────

pub struct KanbanStore {
    conn: Mutex<Connection>,
}

impl KanbanStore {
    /// Open or create a kanban database at the given path.
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Session(format!("Cannot create kanban DB dir: {e}")))?;
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

    /// Open an in-memory kanban store (for tests).
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
            "CREATE TABLE IF NOT EXISTS kanban_boards (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS kanban_columns (
                id        TEXT PRIMARY KEY,
                board_id  TEXT NOT NULL REFERENCES kanban_boards(id) ON DELETE CASCADE,
                name      TEXT NOT NULL,
                position  INTEGER NOT NULL,
                wip_limit INTEGER,
                UNIQUE(board_id, name)
            );

            CREATE TABLE IF NOT EXISTS kanban_cards (
                id          TEXT PRIMARY KEY,
                column_id   TEXT NOT NULL REFERENCES kanban_columns(id) ON DELETE CASCADE,
                board_id    TEXT NOT NULL REFERENCES kanban_boards(id) ON DELETE CASCADE,
                title       TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                priority    TEXT NOT NULL DEFAULT 'medium',
                assignee    TEXT,
                tags        TEXT NOT NULL DEFAULT '[]',
                due_date    TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_cards_column ON kanban_cards(column_id);
            CREATE INDEX IF NOT EXISTS idx_cards_board   ON kanban_cards(board_id);
            CREATE INDEX IF NOT EXISTS idx_cols_board    ON kanban_columns(board_id);",
        )?;
        Ok(())
    }

    // ── Boards ────────────────────────────────────────────────────────

    pub fn create_board(&self, name: &str, description: &str) -> Result<KanbanBoard> {
        let conn = self.lock();
        let id = ulid::Ulid::new().to_string();
        conn.execute(
            "INSERT INTO kanban_boards (id, name, description) VALUES (?1, ?2, ?3)",
            params![id, name, description],
        )?;

        // Auto-create default columns
        let defaults = ["backlog", "todo", "in_progress", "review", "done"];
        for (i, col_name) in defaults.iter().enumerate() {
            let col_id = ulid::Ulid::new().to_string();
            conn.execute(
                "INSERT INTO kanban_columns (id, board_id, name, position) VALUES (?1, ?2, ?3, ?4)",
                params![col_id, id, col_name, i as i64],
            )?;
        }
        drop(conn);
        self.get_board(&id)
    }

    pub fn get_board(&self, id: &str) -> Result<KanbanBoard> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, name, description, created_at, updated_at FROM kanban_boards WHERE id = ?1",
            params![id],
            |row| {
                Ok(KanbanBoard {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .map_err(|e| Error::Session(format!("Board not found: {e}")))
    }

    pub fn list_boards(&self) -> Result<Vec<KanbanBoard>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, created_at, updated_at FROM kanban_boards ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(KanbanBoard {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn delete_board(&self, id: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM kanban_boards WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ── Columns ───────────────────────────────────────────────────────

    pub fn add_column(
        &self,
        board_id: &str,
        name: &str,
        wip_limit: Option<usize>,
    ) -> Result<KanbanColumn> {
        let conn = self.lock();
        // Auto-position at the end
        let position: usize = conn
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM kanban_columns WHERE board_id = ?1",
                params![board_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let id = ulid::Ulid::new().to_string();
        conn.execute(
            "INSERT INTO kanban_columns (id, board_id, name, position, wip_limit) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, board_id, name, position as i64, wip_limit.map(|v| v as i64)],
        )?;

        Ok(KanbanColumn {
            id,
            board_id: board_id.to_string(),
            name: name.to_string(),
            position,
            wip_limit,
        })
    }

    pub fn list_columns(&self, board_id: &str) -> Result<Vec<KanbanColumn>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, board_id, name, position, wip_limit FROM kanban_columns WHERE board_id = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map(params![board_id], |row| {
            Ok(KanbanColumn {
                id: row.get(0)?,
                board_id: row.get(1)?,
                name: row.get(2)?,
                position: row.get::<_, i64>(3)? as usize,
                wip_limit: row.get::<_, Option<i64>>(4)?.map(|v| v as usize),
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ── Cards ─────────────────────────────────────────────────────────

    pub fn add_card(
        &self,
        board_id: &str,
        column_id: &str,
        title: &str,
        description: &str,
        priority: Priority,
        assignee: Option<&str>,
        tags: &[String],
        due_date: Option<&str>,
    ) -> Result<KanbanCard> {
        let conn = self.lock();

        // Validate column belongs to board
        conn.query_row(
            "SELECT id FROM kanban_columns WHERE id = ?1 AND board_id = ?2",
            params![column_id, board_id],
            |_| Ok(()),
        )
        .map_err(|_| Error::Session("Column does not belong to this board".into()))?;

        // WIP check
        let wip_limit: Option<i64> = conn.query_row(
            "SELECT wip_limit FROM kanban_columns WHERE id = ?1",
            params![column_id],
            |row| row.get(0),
        )?;
        if let Some(limit) = wip_limit {
            let count: usize = conn.query_row(
                "SELECT COUNT(*) FROM kanban_cards WHERE column_id = ?1",
                params![column_id],
                |row| row.get(0),
            )?;
            if count >= limit as usize {
                return Err(Error::Session(format!(
                    "WIP limit reached: {count}/{limit} cards in this column"
                )));
            }
        }

        let id = ulid::Ulid::new().to_string();
        let tags_json = serde_json::to_string(tags).unwrap_or_default();

        conn.execute(
            "INSERT INTO kanban_cards (id, column_id, board_id, title, description, priority, assignee, tags, due_date)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                column_id,
                board_id,
                title,
                description,
                priority.as_str(),
                assignee,
                tags_json,
                due_date,
            ],
        )?;
        drop(conn);
        self.get_card(&id)
    }

    pub fn get_card(&self, id: &str) -> Result<KanbanCard> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, column_id, board_id, title, description, priority, assignee, tags, due_date, created_at, updated_at
             FROM kanban_cards WHERE id = ?1",
            params![id],
            |row| {
                let tags_str: String = row.get(7)?;
                let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
                Ok(KanbanCard {
                    id: row.get(0)?,
                    column_id: row.get(1)?,
                    board_id: row.get(2)?,
                    title: row.get(3)?,
                    description: row.get(4)?,
                    priority: Priority::from_str(&row.get::<_, String>(5)?),
                    assignee: row.get(6)?,
                    tags,
                    due_date: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            },
        )
        .map_err(|e| Error::Session(format!("Card not found: {e}")))
    }

    pub fn move_card(&self, card_id: &str, to_column_id: &str) -> Result<KanbanCard> {
        let conn = self.lock();

        // Validate target column exists
        let target_board: String = conn
            .query_row(
                "SELECT board_id FROM kanban_columns WHERE id = ?1",
                params![to_column_id],
                |row| row.get(0),
            )
            .map_err(|_| Error::Session("Target column not found".into()))?;

        // Validate card exists and belongs to same board
        let card_board: String = conn
            .query_row(
                "SELECT board_id FROM kanban_cards WHERE id = ?1",
                params![card_id],
                |row| row.get(0),
            )
            .map_err(|_| Error::Session("Card not found".into()))?;

        if card_board != target_board {
            return Err(Error::Session(
                "Cannot move card to a different board's column".into(),
            ));
        }

        // WIP check on target column
        let wip_limit: Option<i64> = conn.query_row(
            "SELECT wip_limit FROM kanban_columns WHERE id = ?1",
            params![to_column_id],
            |row| row.get(0),
        )?;
        if let Some(limit) = wip_limit {
            let count: usize = conn.query_row(
                "SELECT COUNT(*) FROM kanban_cards WHERE column_id = ?1 AND id != ?2",
                params![to_column_id, card_id],
                |row| row.get(0),
            )?;
            if count >= limit as usize {
                return Err(Error::Session(format!(
                    "WIP limit reached: {count}/{limit} cards in target column"
                )));
            }
        }

        conn.execute(
            "UPDATE kanban_cards SET column_id = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![to_column_id, card_id],
        )?;
        drop(conn);
        self.get_card(card_id)
    }

    pub fn update_card(
        &self,
        card_id: &str,
        title: Option<&str>,
        description: Option<&str>,
        priority: Option<&str>,
        assignee: Option<Option<&str>>,
        tags: Option<&[String]>,
        due_date: Option<Option<&str>>,
    ) -> Result<KanbanCard> {
        let conn = self.lock();
        let mut sets: Vec<String> = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(v) = title {
            sets.push(format!("title = ?{}", sets.len() + 1));
            param_values.push(Box::new(v.to_string()));
        }
        if let Some(v) = description {
            sets.push(format!("description = ?{}", sets.len() + 1));
            param_values.push(Box::new(v.to_string()));
        }
        if let Some(v) = priority {
            sets.push(format!("priority = ?{}", sets.len() + 1));
            param_values.push(Box::new(Priority::from_str(v).as_str().to_string()));
        }
        if let Some(v) = assignee {
            sets.push(format!("assignee = ?{}", sets.len() + 1));
            param_values.push(Box::new(v.map(|s| s.to_string())));
        }
        if let Some(v) = tags {
            sets.push(format!("tags = ?{}", sets.len() + 1));
            param_values.push(Box::new(serde_json::to_string(v).unwrap_or_default()));
        }
        if let Some(v) = due_date {
            sets.push(format!("due_date = ?{}", sets.len() + 1));
            param_values.push(Box::new(v.map(|s| s.to_string())));
        }

        if sets.is_empty() {
            drop(conn);
            return self.get_card(card_id);
        }

        sets.push("updated_at = datetime('now')".to_string());

        let card_id_param_idx = param_values.len() + 1;
        let sql = format!(
            "UPDATE kanban_cards SET {} WHERE id = ?{}",
            sets.join(", "),
            card_id_param_idx
        );

        let mut stmt = conn.prepare(&sql)?;
        let mut params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let card_id_owned = card_id.to_string();
        params_refs.push(&card_id_owned);
        stmt.execute(params_refs.as_slice())?;
        drop(stmt);
        drop(conn);
        self.get_card(card_id)
    }

    pub fn delete_card(&self, card_id: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM kanban_cards WHERE id = ?1", params![card_id])?;
        Ok(())
    }

    pub fn list_cards(
        &self,
        board_id: Option<&str>,
        column_id: Option<&str>,
        assignee: Option<&str>,
    ) -> Result<Vec<KanbanCard>> {
        let conn = self.lock();
        let (where_clause, params_vec) = build_card_filter(board_id, column_id, assignee);
        let mut params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        let sql = format!(
            "SELECT id, column_id, board_id, title, description, priority, assignee, tags,
                    due_date, created_at, updated_at
             FROM kanban_cards {} ORDER BY
                CASE priority
                    WHEN 'critical' THEN 1
                    WHEN 'high' THEN 2
                    WHEN 'medium' THEN 3
                    WHEN 'low' THEN 4
                END, created_at ASC",
            where_clause
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            let tags_str: String = row.get(7)?;
            Ok(KanbanCard {
                id: row.get(0)?,
                column_id: row.get(1)?,
                board_id: row.get(2)?,
                title: row.get(3)?,
                description: row.get(4)?,
                priority: Priority::from_str(&row.get::<_, String>(5)?),
                assignee: row.get(6)?,
                tags: serde_json::from_str(&tags_str).unwrap_or_default(),
                due_date: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ── Stats ─────────────────────────────────────────────────────────

    pub fn get_board_stats(&self, board_id: &str) -> Result<BoardStats> {
        let board = self.get_board(board_id)?;
        let columns = self.list_columns(board_id)?;
        let conn = self.lock();
        let total: usize = conn.query_row(
            "SELECT COUNT(*) FROM kanban_cards WHERE board_id = ?1",
            params![board_id],
            |row| row.get(0),
        )?;
        drop(conn);

        let mut col_stats = Vec::new();
        for col in &columns {
            let cards = self.list_cards(None, Some(&col.id), None)?;
            let count = cards.len();
            col_stats.push(ColumnStats {
                column_id: col.id.clone(),
                column_name: col.name.clone(),
                card_count: count,
                wip_limit: col.wip_limit,
                wip_breached: col.wip_limit.map_or(false, |lim| count > lim),
                cards,
            });
        }

        Ok(BoardStats {
            board_id: board.id,
            board_name: board.name,
            total_cards: total,
            columns: col_stats,
        })
    }
}

/// Build a WHERE clause + params for flexible card listing.
fn build_card_filter(
    board_id: Option<&str>,
    column_id: Option<&str>,
    assignee: Option<&str>,
) -> (String, Vec<String>) {
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let mut idx = 1usize;

    if let Some(b) = board_id {
        conditions.push(format!("board_id = ?{idx}"));
        idx += 1;
        params.push(b.to_string());
    }
    if let Some(c) = column_id {
        conditions.push(format!("column_id = ?{idx}"));
        idx += 1;
        params.push(c.to_string());
    }
    if let Some(a) = assignee {
        conditions.push(format!("assignee = ?{idx}"));
        params.push(a.to_string());
    }

    let clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    (clause, params)
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> KanbanStore {
        KanbanStore::open_in_memory().unwrap()
    }

    #[test]
    fn test_create_board_with_default_columns() {
        let store = test_store();
        let board = store.create_board("Sprint 1", "First sprint").unwrap();

        assert_eq!(board.name, "Sprint 1");
        assert_eq!(board.description, "First sprint");

        let cols = store.list_columns(&board.id).unwrap();
        assert_eq!(cols.len(), 5);
        assert_eq!(cols[0].name, "backlog");
        assert_eq!(cols[4].name, "done");
    }

    #[test]
    fn test_add_and_move_card() {
        let store = test_store();
        let board = store.create_board("Test Board", "").unwrap();
        let cols = store.list_columns(&board.id).unwrap();
        let backlog_id = &cols[0].id;
        let todo_id = &cols[1].id;

        let card = store
            .add_card(
                &board.id,
                backlog_id,
                "Fix login bug",
                "Users cannot login with SSO",
                Priority::High,
                Some("agent-1"),
                &["bug".into(), "auth".into()],
                Some("2026-06-15"),
            )
            .unwrap();

        assert_eq!(card.title, "Fix login bug");
        assert_eq!(card.priority, Priority::High);
        assert_eq!(card.assignee.as_deref(), Some("agent-1"));
        assert_eq!(card.tags, vec!["bug", "auth"]);

        // Move card to todo
        let moved = store.move_card(&card.id, todo_id).unwrap();
        assert_eq!(moved.column_id, *todo_id);

        // Card is now in todo column
        let todo_cards = store.list_cards(None, Some(todo_id), None).unwrap();
        assert_eq!(todo_cards.len(), 1);
        assert_eq!(todo_cards[0].id, card.id);
    }

    #[test]
    fn test_wip_limit_enforcement() {
        let store = test_store();
        let board = store.create_board("WIP Test", "").unwrap();

        // Add a column with WIP limit of 2
        let col = store.add_column(&board.id, "testing", Some(2)).unwrap();

        // Add 2 cards (at limit)
        store
            .add_card(
                &board.id,
                &col.id,
                "Card 1",
                "",
                Priority::Medium,
                None,
                &[],
                None,
            )
            .unwrap();
        store
            .add_card(
                &board.id,
                &col.id,
                "Card 2",
                "",
                Priority::Medium,
                None,
                &[],
                None,
            )
            .unwrap();

        // 3rd card should fail
        let result = store.add_card(
            &board.id,
            &col.id,
            "Card 3",
            "",
            Priority::Medium,
            None,
            &[],
            None,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("WIP limit reached"));
    }

    #[test]
    fn test_move_card_wip_block() {
        let store = test_store();
        let board = store.create_board("Move WIP Test", "").unwrap();
        let cols = store.list_columns(&board.id).unwrap();

        // Set WIP limit of 1 on todo column
        let todo_id = &cols[1].id;
        {
            let conn = store.lock();
            conn.execute(
                "UPDATE kanban_columns SET wip_limit = 1 WHERE id = ?1",
                params![todo_id],
            )
            .unwrap();
        }

        // Add card to backlog
        let card = store
            .add_card(
                &board.id,
                &cols[0].id,
                "Task",
                "",
                Priority::Medium,
                None,
                &[],
                None,
            )
            .unwrap();

        // Add another card directly to todo (fills the WIP)
        store
            .add_card(
                &board.id,
                todo_id,
                "Blocking",
                "",
                Priority::Medium,
                None,
                &[],
                None,
            )
            .unwrap();

        // Move first card to todo should fail
        let result = store.move_card(&card.id, todo_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_card_fields() {
        let store = test_store();
        let board = store.create_board("Update Test", "").unwrap();
        let cols = store.list_columns(&board.id).unwrap();
        let card = store
            .add_card(
                &board.id,
                &cols[0].id,
                "Old Title",
                "Old desc",
                Priority::Low,
                None,
                &[],
                None,
            )
            .unwrap();

        let updated = store
            .update_card(
                &card.id,
                Some("New Title"),
                Some("New desc"),
                Some("critical"),
                Some(Some("agent-7")),
                Some(&["urgent".into()]),
                Some(Some("2026-07-01")),
            )
            .unwrap();

        assert_eq!(updated.title, "New Title");
        assert_eq!(updated.description, "New desc");
        assert_eq!(updated.priority, Priority::Critical);
        assert_eq!(updated.assignee.as_deref(), Some("agent-7"));
        assert_eq!(updated.tags, vec!["urgent"]);
        assert_eq!(updated.due_date.as_deref(), Some("2026-07-01"));
    }

    #[test]
    fn test_board_stats() {
        let store = test_store();
        let board = store.create_board("Stats Test", "").unwrap();
        let cols = store.list_columns(&board.id).unwrap();

        store
            .add_card(
                &board.id,
                &cols[0].id,
                "A",
                "",
                Priority::High,
                None,
                &[],
                None,
            )
            .unwrap();
        store
            .add_card(
                &board.id,
                &cols[0].id,
                "B",
                "",
                Priority::Low,
                None,
                &[],
                None,
            )
            .unwrap();
        store
            .add_card(
                &board.id,
                &cols[1].id,
                "C",
                "",
                Priority::Medium,
                None,
                &[],
                None,
            )
            .unwrap();

        let stats = store.get_board_stats(&board.id).unwrap();
        assert_eq!(stats.total_cards, 3);
        assert_eq!(stats.columns.len(), 5);

        let backlog = &stats.columns[0];
        assert_eq!(backlog.card_count, 2);
        assert_eq!(backlog.cards[0].priority, Priority::High); // higher prio first

        let todo_col = &stats.columns[1];
        assert_eq!(todo_col.card_count, 1);

        let done = &stats.columns[4];
        assert_eq!(done.card_count, 0);
    }

    #[test]
    fn test_list_boards() {
        let store = test_store();
        store.create_board("Board A", "").unwrap();
        store.create_board("Board B", "").unwrap();

        let boards = store.list_boards().unwrap();
        assert_eq!(boards.len(), 2);
    }

    #[test]
    fn test_delete_board_cascades() {
        let store = test_store();
        let board = store.create_board("To Delete", "").unwrap();
        let cols = store.list_columns(&board.id).unwrap();
        store
            .add_card(
                &board.id,
                &cols[0].id,
                "Task",
                "",
                Priority::Medium,
                None,
                &[],
                None,
            )
            .unwrap();

        store.delete_board(&board.id).unwrap();

        // Board gone
        assert!(store.get_board(&board.id).is_err());
        // Cards gone (CASCADE)
        let cards = store.list_cards(None, None, None).unwrap();
        assert_eq!(cards.len(), 0);
    }

    #[test]
    fn test_list_cards_filtered() {
        let store = test_store();
        let board = store.create_board("Filter Test", "").unwrap();
        let cols = store.list_columns(&board.id).unwrap();

        store
            .add_card(
                &board.id,
                &cols[0].id,
                "Task 1",
                "",
                Priority::Medium,
                Some("alice"),
                &[],
                None,
            )
            .unwrap();
        store
            .add_card(
                &board.id,
                &cols[1].id,
                "Task 2",
                "",
                Priority::Medium,
                Some("bob"),
                &[],
                None,
            )
            .unwrap();

        // Filter by assignee
        let alice_cards = store.list_cards(None, None, Some("alice")).unwrap();
        assert_eq!(alice_cards.len(), 1);
        assert_eq!(alice_cards[0].title, "Task 1");

        // Filter by board + column
        let col0_cards = store
            .list_cards(Some(&board.id), Some(&cols[0].id), None)
            .unwrap();
        assert_eq!(col0_cards.len(), 1);
    }

    #[test]
    fn test_delete_card() {
        let store = test_store();
        let board = store.create_board("Card Delete", "").unwrap();
        let cols = store.list_columns(&board.id).unwrap();
        let card = store
            .add_card(
                &board.id,
                &cols[0].id,
                "X",
                "",
                Priority::Medium,
                None,
                &[],
                None,
            )
            .unwrap();

        store.delete_card(&card.id).unwrap();
        assert!(store.get_card(&card.id).is_err());
    }
}
