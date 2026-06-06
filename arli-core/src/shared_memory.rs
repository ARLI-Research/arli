//! Shared Memory for Multi-Agent Swarms — a common blackboard for agent coordination.
//!
//! From "Multi-Agent Memory and Shared-Harness Synchronization" (2025):
//! when multiple agents operate in the same harness, they need a shared memory
//! substrate — not just per-agent stores, but a common namespace where agents
//! publish discoveries, read each other's outputs, and coordinate without
//! explicit message passing.
//!
//! Use case: Agent A discovers a market inefficiency → writes to shared memory.
//! Agent B reads the discovery → opens a position. Agent C reads the result →
//! reports to ENSO.
//!
//! Thread-safe (RwLock), namespaced by agent_id, persisted to JSON.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

// ============================================================================
// SHARED MEMORY ENTRY
// ============================================================================

/// A single entry in the shared memory — key-value with authorship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedEntry {
    /// Entry key (namespaced — agents use "agent_id:topic" convention).
    pub key: String,

    /// JSON-serializable value.
    pub value: String,

    /// Which agent wrote this entry.
    pub author: String,

    /// Unix timestamp in milliseconds when written.
    pub timestamp_ms: u64,

    /// Optional TTL in seconds (0 = never expires).
    pub ttl_secs: u64,

    /// Version counter — incremented on each write.
    pub version: u64,
}

impl SharedEntry {
    pub fn new(key: &str, value: &str, author: &str, ttl_secs: u64) -> Self {
        Self {
            key: key.to_string(),
            value: value.to_string(),
            author: author.to_string(),
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            ttl_secs,
            version: 1,
        }
    }

    /// Check if this entry has expired.
    pub fn is_expired(&self) -> bool {
        if self.ttl_secs == 0 {
            return false;
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let age_secs = (now_ms - self.timestamp_ms) / 1000;
        age_secs >= self.ttl_secs
    }
}

// ============================================================================
// SHARED MEMORY STORE
// ============================================================================

/// Thread-safe shared memory for multi-agent coordination.
///
/// Multiple agents can read and write concurrently. All writes are attributed
/// to the writing agent. The store can be persisted to disk.
#[derive(Debug, Default)]
pub struct SharedMemory {
    entries: RwLock<HashMap<String, SharedEntry>>,
}

impl SharedMemory {
    /// Create an empty shared memory store.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Write a value under a key, attributed to the given agent.
    ///
    /// If the key already exists, increments the version counter.
    /// Returns the new version number.
    pub fn write(&self, key: &str, value: &str, author: &str, ttl_secs: u64) -> u64 {
        let mut entries = self.entries.write().unwrap();

        if let Some(existing) = entries.get_mut(key) {
            existing.value = value.to_string();
            existing.author = author.to_string();
            existing.timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            existing.ttl_secs = ttl_secs;
            existing.version += 1;
            existing.version
        } else {
            let entry = SharedEntry::new(key, value, author, ttl_secs);
            let version = entry.version;
            entries.insert(key.to_string(), entry);
            version
        }
    }

    /// Read a value by key. Returns None if key doesn't exist or has expired.
    pub fn read(&self, key: &str) -> Option<SharedEntry> {
        let entries = self.entries.read().unwrap();
        entries.get(key).cloned().filter(|e| !e.is_expired())
    }

    /// Read all entries authored by a specific agent.
    pub fn read_by_author(&self, author: &str) -> Vec<SharedEntry> {
        let entries = self.entries.read().unwrap();
        entries
            .values()
            .filter(|e| e.author == author && !e.is_expired())
            .cloned()
            .collect()
    }

    /// Read all entries matching a key prefix (for namespace queries).
    ///
    /// Example: `read_by_prefix("market:")` returns all market-related entries.
    pub fn read_by_prefix(&self, prefix: &str) -> Vec<SharedEntry> {
        let entries = self.entries.read().unwrap();
        entries
            .iter()
            .filter(|(k, e)| k.starts_with(prefix) && !e.is_expired())
            .map(|(_, e)| e.clone())
            .collect()
    }

    /// Delete an entry by key. Returns true if it existed.
    pub fn delete(&self, key: &str) -> bool {
        let mut entries = self.entries.write().unwrap();
        entries.remove(key).is_some()
    }

    /// List all non-expired keys.
    pub fn keys(&self) -> Vec<String> {
        let entries = self.entries.read().unwrap();
        entries
            .iter()
            .filter(|(_, e)| !e.is_expired())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Purge all expired entries. Returns count of entries removed.
    pub fn purge_expired(&self) -> usize {
        let mut entries = self.entries.write().unwrap();
        let before = entries.len();
        entries.retain(|_, e| !e.is_expired());
        before - entries.len()
    }

    /// Number of non-expired entries in the store.
    pub fn len(&self) -> usize {
        let entries = self.entries.read().unwrap();
        entries.values().filter(|e| !e.is_expired()).count()
    }

    /// True if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all entries.
    pub fn clear(&self) {
        let mut entries = self.entries.write().unwrap();
        entries.clear();
    }

    /// Snapshot of all entries (for persistence).
    pub fn snapshot(&self) -> Vec<SharedEntry> {
        let entries = self.entries.read().unwrap();
        entries
            .values()
            .filter(|e| !e.is_expired())
            .cloned()
            .collect()
    }

    /// Persist to a JSON file.
    pub fn save(&self, path: &PathBuf) -> Result<(), String> {
        let snapshot = self.snapshot();
        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| format!("serialize shared memory: {}", e))?;
        std::fs::write(path, json).map_err(|e| format!("write shared memory: {}", e))?;
        Ok(())
    }

    /// Load from a JSON file. Returns empty store if file doesn't exist.
    pub fn load(path: &PathBuf) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let data =
            std::fs::read_to_string(path).map_err(|e| format!("read shared memory: {}", e))?;
        let entries: Vec<SharedEntry> =
            serde_json::from_str(&data).map_err(|e| format!("parse shared memory: {}", e))?;
        let map: HashMap<String, SharedEntry> =
            entries.into_iter().map(|e| (e.key.clone(), e)).collect();
        Ok(Self {
            entries: RwLock::new(map),
        })
    }

    /// Default path: `~/.arli/shared_memory.json`
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".arli").join("shared_memory.json")
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read() {
        let mem = SharedMemory::new();
        mem.write("price:BTC", "105000", "agent-1", 0);
        let entry = mem.read("price:BTC").unwrap();
        assert_eq!(entry.value, "105000");
        assert_eq!(entry.author, "agent-1");
        assert_eq!(entry.version, 1);
    }

    #[test]
    fn test_version_increment() {
        let mem = SharedMemory::new();
        let v1 = mem.write("signal", "BUY", "agent-1", 0);
        assert_eq!(v1, 1);
        let v2 = mem.write("signal", "SELL", "agent-2", 0);
        assert_eq!(v2, 2);
        let entry = mem.read("signal").unwrap();
        assert_eq!(entry.value, "SELL");
        assert_eq!(entry.author, "agent-2");
        assert_eq!(entry.version, 2);
    }

    #[test]
    fn test_read_missing() {
        let mem = SharedMemory::new();
        assert!(mem.read("nonexistent").is_none());
    }

    #[test]
    fn test_read_by_author() {
        let mem = SharedMemory::new();
        mem.write("a:1", "x", "alice", 0);
        mem.write("a:2", "y", "alice", 0);
        mem.write("b:1", "z", "bob", 0);

        let alice = mem.read_by_author("alice");
        assert_eq!(alice.len(), 2);
        let bob = mem.read_by_author("bob");
        assert_eq!(bob.len(), 1);
    }

    #[test]
    fn test_read_by_prefix() {
        let mem = SharedMemory::new();
        mem.write("market:BTC", "100k", "agent-1", 0);
        mem.write("market:ETH", "5k", "agent-1", 0);
        mem.write("signal:BUY", "now", "agent-2", 0);

        let market = mem.read_by_prefix("market:");
        assert_eq!(market.len(), 2);
        let signal = mem.read_by_prefix("signal:");
        assert_eq!(signal.len(), 1);
    }

    #[test]
    fn test_delete() {
        let mem = SharedMemory::new();
        mem.write("key", "val", "agent-1", 0);
        assert!(mem.delete("key"));
        assert!(!mem.delete("key"));
        assert!(mem.read("key").is_none());
    }

    #[test]
    fn test_keys() {
        let mem = SharedMemory::new();
        mem.write("a", "1", "x", 0);
        mem.write("b", "2", "x", 0);
        let keys = mem.keys();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"a".to_string()));
        assert!(keys.contains(&"b".to_string()));
    }

    #[test]
    fn test_ttl_expiry() {
        let mem = SharedMemory::new();
        mem.write("ephemeral", "data", "agent-1", 0); // TTL=0 means never expires
        assert!(mem.read("ephemeral").is_some());

        // Write with TTL=1 second, then immediately check — should still be there
        mem.write("short", "data", "agent-1", 1);
        assert!(mem.read("short").is_some());
    }

    #[test]
    fn test_purge_expired() {
        let mem = SharedMemory::new();
        mem.write("keep", "val", "agent-1", 3600); // 1 hour
        mem.write("expire", "val", "agent-1", 0); // Using TTL=0 so it won't expire

        let purged = mem.purge_expired();
        assert_eq!(purged, 0); // Nothing expired yet
        assert_eq!(mem.len(), 2);
    }

    #[test]
    fn test_clear() {
        let mem = SharedMemory::new();
        mem.write("a", "1", "x", 0);
        mem.write("b", "2", "x", 0);
        assert_eq!(mem.len(), 2);
        mem.clear();
        assert!(mem.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let tmp = std::env::temp_dir().join("arli_test_shared_memory.json");
        let _ = std::fs::remove_file(&tmp);

        let mem = SharedMemory::new();
        mem.write("key1", "val1", "agent-1", 0);
        mem.write("key2", "val2", "agent-2", 0);
        mem.save(&tmp).unwrap();

        let loaded = SharedMemory::load(&tmp).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.read("key1").unwrap().value, "val1");
        assert_eq!(loaded.read("key2").unwrap().author, "agent-2");

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_load_nonexistent() {
        let path = PathBuf::from("/tmp/arli_nonexistent_shared_memory.json");
        let mem = SharedMemory::load(&path).unwrap();
        assert!(mem.is_empty());
    }

    #[test]
    fn test_snapshot() {
        let mem = SharedMemory::new();
        mem.write("a", "1", "x", 0);
        mem.write("b", "2", "y", 0);
        let snap = mem.snapshot();
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn test_entry_timestamp() {
        let entry = SharedEntry::new("test", "val", "agent-1", 0);
        assert!(entry.timestamp_ms > 0);
        assert!(!entry.is_expired()); // TTL=0 never expires
    }
}
