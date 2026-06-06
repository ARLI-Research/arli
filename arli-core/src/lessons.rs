//! Experiential Memory — lessons learned from failures, applied automatically.
//!
//! From "Code as Agent Harness" §3.2.3: the agent should learn from errors.
//! When a failure pattern repeats, apply the known fix without re-trying.
//!
//! Stores `~/.arli/lessons.json` — a simple key-value map of
//! error-pattern → fix, persisted across runs.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================================
// EXPERIENTIAL MEMORY
// ============================================================================

/// A single lesson — one error pattern and its fix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    /// Regex-compatible error pattern (matched case-insensitively against error messages).
    pub error_pattern: String,

    /// Human-readable description of the fix that worked.
    pub fix_description: String,

    /// ISO-8601 timestamp when this lesson was learned.
    pub learned_at: String,

    /// How many times this fix has been applied successfully.
    #[serde(default)]
    pub times_applied: u32,
}

/// Persistent store of failure → fix patterns.
///
/// Loaded from `~/.arli/lessons.json` at startup, saved after each new lesson.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExperientialMemory {
    /// Lessons keyed by a normalized pattern (lowercased, trimmed).
    #[serde(default)]
    pub lessons: Vec<Lesson>,
}

impl ExperientialMemory {
    /// Default path: `~/.arli/lessons.json`
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".arli").join("lessons.json")
    }

    /// Load lessons from disk. Returns empty if file doesn't exist.
    pub fn load(path: &PathBuf) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data =
            std::fs::read_to_string(path).map_err(|e| format!("read lessons file: {}", e))?;
        serde_json::from_str(&data).map_err(|e| format!("parse lessons: {}", e))
    }

    /// Save lessons to disk. Creates parent directory if needed.
    pub fn save(&self, path: &PathBuf) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {}", e))?;
        }
        let json =
            serde_json::to_string_pretty(self).map_err(|e| format!("serialize lessons: {}", e))?;
        std::fs::write(path, &json).map_err(|e| format!("write lessons: {}", e))
    }

    /// Search for a known fix matching this error message.
    ///
    /// Returns `Some(fix_description)` if an error pattern matches,
    /// or `None` if no lesson covers this error.
    pub fn find_fix(&self, error_msg: &str) -> Option<String> {
        let normalized = error_msg.to_lowercase();
        for lesson in &self.lessons {
            if normalized.contains(&lesson.error_pattern.to_lowercase()) {
                return Some(lesson.fix_description.clone());
            }
        }
        None
    }

    /// Record a new lesson: error pattern → fix.
    ///
    /// If the same pattern already exists, bumps `times_applied`.
    /// Otherwise, appends a new lesson.
    pub fn record(&mut self, error_pattern: &str, fix_description: &str) {
        let pattern_lower = error_pattern.to_lowercase();

        // Check if we already have this pattern
        for lesson in &mut self.lessons {
            if lesson.error_pattern.to_lowercase() == pattern_lower {
                lesson.times_applied += 1;
                return;
            }
        }

        // New lesson
        let now = chrono::Utc::now().to_rfc3339();
        self.lessons.push(Lesson {
            error_pattern: error_pattern.to_string(),
            fix_description: fix_description.to_string(),
            learned_at: now,
            times_applied: 1,
        });
    }

    /// Record a lesson and persist to disk atomically.
    pub fn record_and_save(
        &mut self,
        path: &PathBuf,
        error_pattern: &str,
        fix_description: &str,
    ) -> Result<(), String> {
        self.record(error_pattern, fix_description);
        self.save(path)
    }

    /// Number of stored lessons.
    pub fn lesson_count(&self) -> usize {
        self.lessons.len()
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_memory_is_empty() {
        let mem = ExperientialMemory::default();
        assert_eq!(mem.lesson_count(), 0);
        assert!(mem.find_fix("any error").is_none());
    }

    #[test]
    fn test_record_and_find() {
        let mut mem = ExperientialMemory::default();
        mem.record("expected packfile", "Retry git clone with --depth 1");
        mem.record("no space left on device", "Clean up /tmp and target/");

        let fix = mem.find_fix("fatal: expected packfile");
        assert_eq!(fix, Some("Retry git clone with --depth 1".into()));

        let fix2 = mem.find_fix("No space left on device (os error 28)");
        assert_eq!(fix2, Some("Clean up /tmp and target/".into()));

        // No match
        assert!(mem.find_fix("segfault at 0x0").is_none());
    }

    #[test]
    fn test_case_insensitive_match() {
        let mut mem = ExperientialMemory::default();
        mem.record("not in the approved", "Re-register agent on ENSO");

        let fix = mem.find_fix("ARLI BINARY HASH 'abc' IS NOT IN THE APPROVED registry");
        assert!(fix.is_some());
    }

    #[test]
    fn test_duplicate_pattern_bumps_counter() {
        let mut mem = ExperientialMemory::default();
        mem.record("timeout", "Increase timeout");
        mem.record("timeout", "Increase timeout"); // same pattern

        assert_eq!(mem.lesson_count(), 1);
        assert_eq!(mem.lessons[0].times_applied, 2);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut mem = ExperientialMemory::default();
        mem.record("contract not active", "Activate contract on ENSO canister");

        let json = serde_json::to_string(&mem).unwrap();
        let mem2: ExperientialMemory = serde_json::from_str(&json).unwrap();

        assert_eq!(mem2.lesson_count(), 1);
        let fix = mem2.find_fix("Contract is not in Active status — contract not active");
        assert!(fix.is_some());
    }

    #[test]
    fn test_save_and_load_to_disk() {
        let tmp = std::env::temp_dir().join("arli_test_lessons.json");
        let _ = std::fs::remove_file(&tmp);

        let mut mem = ExperientialMemory::default();
        mem.record("sandbox policy too strict", "Relax Landlock rules for /proc");
        mem.save(&tmp).unwrap();

        let loaded = ExperientialMemory::load(&tmp).unwrap();
        assert_eq!(loaded.lesson_count(), 1);
        let fix = loaded.find_fix("sandbox policy too strict for reading /proc/cpuinfo");
        assert!(fix.is_some());

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_load_nonexistent_returns_empty() {
        let tmp = std::env::temp_dir().join("arli_nonexistent_lessons.json");
        let _ = std::fs::remove_file(&tmp);
        let mem = ExperientialMemory::load(&tmp).unwrap();
        assert_eq!(mem.lesson_count(), 0);
    }

    #[test]
    fn test_multiple_patterns_match_first() {
        let mut mem = ExperientialMemory::default();
        mem.record("expected packfile", "fix-packfile");
        mem.record("git", "generic-git-fix");

        // Both patterns match, but first match wins
        let fix = mem.find_fix("fatal: expected packfile from git");
        assert_eq!(fix, Some("fix-packfile".into()));
    }
}
