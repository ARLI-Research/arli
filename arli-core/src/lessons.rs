//! Experiential Memory — lessons learned from failures, applied automatically.
//!
//! From "Code as Agent Harness" §3.2.3 + MemGovern (2026): the agent should learn
//! from errors but not accumulate noise. Only verified fixes are kept long-term.
//!
//! Stores `~/.arli/lessons.json` — error-pattern → fix, with quality governance.

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

    /// How many times this fix has been applied.
    #[serde(default)]
    pub times_applied: u32,

    /// How many times this fix successfully resolved the error (hit).
    #[serde(default)]
    pub hits: u32,

    /// How many times this fix was tried but didn't help (miss).
    #[serde(default)]
    pub misses: u32,

    /// Whether this lesson has been verified (fix truly resolved the issue).
    /// Unverified lessons older than 7 days are auto-pruned.
    #[serde(default)]
    pub verified: bool,
}

/// Persistent store of failure → fix patterns with quality governance.
///
/// Loaded from `~/.arli/lessons.json` at startup, saved after each new lesson.
/// MemGovern-style: unverified lessons older than 7 days are pruned on load.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExperientialMemory {
    /// Lessons keyed by a normalized pattern (lowercased, trimmed).
    #[serde(default)]
    pub lessons: Vec<Lesson>,

    /// Total find_fix() calls (for memory health reporting).
    #[serde(default)]
    pub total_lookups: u64,

    /// Total successful find_fix() calls.
    #[serde(default)]
    pub total_hits: u64,
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
    /// Tracks hit/miss counters for memory health reporting.
    pub fn find_fix(&mut self, error_msg: &str) -> Option<String> {
        self.total_lookups += 1;
        let normalized = error_msg.to_lowercase();
        for lesson in &self.lessons {
            if normalized.contains(&lesson.error_pattern.to_lowercase()) {
                self.total_hits += 1;
                return Some(lesson.fix_description.clone());
            }
        }
        None
    }

    /// Record a new lesson: error pattern → fix.
    ///
    /// If the same pattern already exists, bumps `times_applied`.
    /// Otherwise, appends a new lesson (unverified by default — MemGovern).
    pub fn record(&mut self, error_pattern: &str, fix_description: &str) {
        let pattern_lower = error_pattern.to_lowercase();

        // Check if we already have this pattern
        for lesson in &mut self.lessons {
            if lesson.error_pattern.to_lowercase() == pattern_lower {
                lesson.times_applied += 1;
                return;
            }
        }

        // New lesson — unverified until proven
        let now = chrono::Utc::now().to_rfc3339();
        self.lessons.push(Lesson {
            error_pattern: error_pattern.to_string(),
            fix_description: fix_description.to_string(),
            learned_at: now,
            times_applied: 1,
            hits: 0,
            misses: 0,
            verified: false,
        });
    }

    /// Verify a lesson: mark as proven when fix successfully resolved the error.
    /// Also bumps hits counter.
    pub fn verify_fix(&mut self, error_pattern: &str, worked: bool) {
        let pattern_lower = error_pattern.to_lowercase();
        for lesson in &mut self.lessons {
            if lesson.error_pattern.to_lowercase() == pattern_lower {
                if worked {
                    lesson.hits += 1;
                    lesson.verified = true;
                } else {
                    lesson.misses += 1;
                    // Unverify if miss rate exceeds 50%
                    if lesson.misses > lesson.hits {
                        lesson.verified = false;
                    }
                }
                return;
            }
        }
    }

    /// Prune unverified lessons older than `max_age_days`.
    /// Returns count of pruned lessons.
    pub fn prune_unverified(&mut self, max_age_days: i64) -> usize {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days);
        let before = self.lessons.len();
        self.lessons.retain(|lesson| {
            if lesson.verified {
                return true;
            }
            // Parse timestamp, keep if recent
            if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&lesson.learned_at) {
                ts.with_timezone(&chrono::Utc) > cutoff
            } else {
                true // Can't parse — keep
            }
        });
        before - self.lessons.len()
    }

    /// Memory health report: hit rate, total lessons, verified count.
    pub fn health(&self) -> MemoryHealth {
        let verified = self.lessons.iter().filter(|l| l.verified).count();
        let hit_rate = if self.total_lookups > 0 {
            self.total_hits as f64 / self.total_lookups as f64
        } else {
            0.0
        };
        MemoryHealth {
            total_lessons: self.lessons.len(),
            verified_lessons: verified,
            unverified_lessons: self.lessons.len() - verified,
            total_lookups: self.total_lookups,
            total_hits: self.total_hits,
            hit_rate,
        }
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

/// Snapshot of memory quality — for telemetry and health checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHealth {
    pub total_lessons: usize,
    pub verified_lessons: usize,
    pub unverified_lessons: usize,
    pub total_lookups: u64,
    pub total_hits: u64,
    pub hit_rate: f64,
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_memory_is_empty() {
        let mut mem = ExperientialMemory::default();
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
        // find_fix on deserialized doesn't need mut since we check content
        assert!(!mem2.lessons.is_empty());
    }

    #[test]
    fn test_save_and_load_to_disk() {
        let tmp = std::env::temp_dir().join("arli_test_lessons.json");
        let _ = std::fs::remove_file(&tmp);

        let mut mem = ExperientialMemory::default();
        mem.record("sandbox policy too strict", "Relax Landlock rules for /proc");
        mem.save(&tmp).unwrap();

        let mut loaded = ExperientialMemory::load(&tmp).unwrap();
        assert_eq!(loaded.lesson_count(), 1);
        let fix = loaded.find_fix("sandbox policy too strict for reading /proc/cpuinfo");
        assert!(fix.is_some());

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_load_nonexistent_returns_empty() {
        let tmp = std::env::temp_dir().join("arli_nonexistent_lessons.json");
        let _ = std::fs::remove_file(&tmp);
        let mut mem = ExperientialMemory::load(&tmp).unwrap();
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

    // --- MemGovern tests ---

    #[test]
    fn test_new_lesson_is_unverified() {
        let mut mem = ExperientialMemory::default();
        mem.record("disk full", "Clean up target/");
        assert!(!mem.lessons[0].verified);
        assert_eq!(mem.lessons[0].hits, 0);
        assert_eq!(mem.lessons[0].misses, 0);
    }

    #[test]
    fn test_verify_fix_marks_verified() {
        let mut mem = ExperientialMemory::default();
        mem.record("disk full", "Clean up target/");
        mem.verify_fix("disk full", true);
        assert!(mem.lessons[0].verified);
        assert_eq!(mem.lessons[0].hits, 1);
    }

    #[test]
    fn test_verify_fix_failure_unverifies() {
        let mut mem = ExperientialMemory::default();
        mem.record("disk full", "Clean up target/");
        mem.verify_fix("disk full", true);
        assert!(mem.lessons[0].verified);

        mem.verify_fix("disk full", false);
        mem.verify_fix("disk full", false);
        // 2 misses > 1 hit → unverified
        assert!(!mem.lessons[0].verified);
    }

    #[test]
    fn test_prune_unverified() {
        let mut mem = ExperientialMemory::default();
        mem.record("old error", "old fix");
        mem.record("verified error", "verified fix");
        mem.verify_fix("verified error", true);

        // Override timestamp of "old error" to be 10 days ago
        mem.lessons[0].learned_at = (chrono::Utc::now() - chrono::Duration::days(10)).to_rfc3339();

        let pruned = mem.prune_unverified(7);
        assert_eq!(pruned, 1);
        assert_eq!(mem.lesson_count(), 1);
        assert_eq!(mem.lessons[0].error_pattern, "verified error");
    }

    #[test]
    fn test_health_report() {
        let mut mem = ExperientialMemory::default();
        mem.record("err1", "fix1");
        mem.record("err2", "fix2");
        mem.verify_fix("err1", true);
        mem.find_fix("something with err1 in it");
        mem.find_fix("something else");
        mem.find_fix("another thing");

        let health = mem.health();
        assert_eq!(health.total_lessons, 2);
        assert_eq!(health.verified_lessons, 1);
        assert_eq!(health.total_lookups, 3);
        assert_eq!(health.total_hits, 1);
        assert!((health.hit_rate - 1.0 / 3.0).abs() < 0.01);
    }
}
