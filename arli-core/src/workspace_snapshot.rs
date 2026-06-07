//! Fault-Tolerant Workspace Snapshots — rollback on failure.
//!
//! From "Fault-Tolerant Sandboxing for AI Coding Agents" (2025):
//! before executing a contract, save workspace state. On failure,
//! rollback to clean state for retry. On success, discard snapshot.
//!
//! Uses git stash when available, falls back to tar-based snapshot.

use std::path::{Path, PathBuf};
use std::process::Command;

// ============================================================================
// WORKSPACE SNAPSHOT
// ============================================================================

/// A saved workspace state that can be rolled back or committed.
///
/// Created before contract execution, rolled back on failure,
/// discarded on success.
#[derive(Debug)]
pub struct WorkspaceSnapshot {
    /// Workspace root directory.
    path: PathBuf,
    /// Git stash reference (if git was used).
    stash_ref: Option<String>,
    /// Tar file path (if tar fallback was used).
    tar_path: Option<PathBuf>,
}

impl WorkspaceSnapshot {
    /// Create a snapshot of the workspace at `path`.
    ///
    /// Prefers `git stash` if the directory is a git repo.
    /// Falls back to `tar` for non-git directories.
    pub fn create(path: &Path) -> Result<Self, String> {
        let path = path.to_path_buf();

        if !path.exists() {
            return Err(format!("workspace does not exist: {}", path.display()));
        }

        // Try git first
        if path.join(".git").exists() {
            match Self::create_git_stash(&path) {
                Ok(stash_ref) => {
                    return Ok(Self {
                        path,
                        stash_ref: Some(stash_ref),
                        tar_path: None,
                    });
                }
                Err(e) => {
                    tracing::warn!("Git stash failed ({}), falling back to tar", e);
                }
            }
        }

        // Fallback: tar
        let tar_path = Self::create_tar(&path)?;
        Ok(Self {
            path,
            stash_ref: None,
            tar_path: Some(tar_path),
        })
    }

    /// Rollback to the saved state (called on execution failure).
    ///
    /// Consumes the snapshot — after rollback, the workspace is clean.
    pub fn rollback(self) -> Result<(), String> {
        if let Some(ref stash_ref) = self.stash_ref {
            self.rollback_git(stash_ref)
        } else if let Some(ref tar_path) = self.tar_path {
            self.rollback_tar(tar_path)
        } else {
            Err("no snapshot data to rollback".into())
        }
    }

    /// Discard the snapshot (called on execution success).
    ///
    /// Consumes the snapshot — workspace changes are permanent.
    pub fn commit(self) -> Result<(), String> {
        if let Some(ref stash_ref) = self.stash_ref {
            // Drop the stash we created
            let output = Command::new("git")
                .args(["stash", "drop", stash_ref])
                .current_dir(&self.path)
                .output()
                .map_err(|e| format!("git stash drop: {}", e))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("git stash drop failed: {}", stderr));
            }
        }
        // For tar snapshots, just delete the temp file
        if let Some(ref tar_path) = self.tar_path {
            let _ = std::fs::remove_file(tar_path);
        }
        Ok(())
    }

    /// Check if the snapshot is valid (has data to restore).
    pub fn is_valid(&self) -> bool {
        self.stash_ref.is_some() || self.tar_path.is_some()
    }

    // --- Git-based snapshot ---

    fn create_git_stash(path: &Path) -> Result<String, String> {
        // Stash everything including untracked files
        let output = Command::new("git")
            .args([
                "stash",
                "push",
                "--include-untracked",
                "-m",
                "arli-fault-tolerance",
            ])
            .current_dir(path)
            .output()
            .map_err(|e| format!("git stash: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // "No local changes to save" is OK — means workspace was clean
            if stderr.contains("No local changes") {
                return Ok("arli-clean-workspace".into());
            }
            return Err(format!("git stash failed: {}", stderr));
        }

        // Get the stash reference
        let output = Command::new("git")
            .args(["stash", "list", "-n", "1", "--format=%gd"])
            .current_dir(path)
            .output()
            .map_err(|e| format!("git stash list: {}", e))?;

        let ref_name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if ref_name.is_empty() {
            // No changes were stashed — workspace was clean
            return Ok("arli-clean-workspace".into());
        }

        Ok(ref_name)
    }

    fn rollback_git(&self, stash_ref: &str) -> Result<(), String> {
        if stash_ref == "arli-clean-workspace" {
            // Workspace was clean — just reset any new changes
            let _ = Command::new("git")
                .args(["checkout", "--", "."])
                .current_dir(&self.path)
                .output();
            let _ = Command::new("git")
                .args(["clean", "-fd"])
                .current_dir(&self.path)
                .output();
            return Ok(());
        }

        // Pop the stash to restore previous state
        let output = Command::new("git")
            .args(["stash", "pop", stash_ref])
            .current_dir(&self.path)
            .output()
            .map_err(|e| format!("git stash pop: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git stash pop failed: {}", stderr));
        }

        Ok(())
    }

    // --- Tar-based snapshot (fallback for non-git workspaces) ---

    fn create_tar(path: &Path) -> Result<PathBuf, String> {
        // Use unique subdirectory per workspace to avoid cross-test conflicts
        let workspace_name = path.file_name().unwrap_or_default().to_string_lossy();
        let temp_dir = std::env::temp_dir().join(format!("arli_snapshots_{}", workspace_name));
        std::fs::create_dir_all(&temp_dir).map_err(|e| format!("create snapshot dir: {}", e))?;

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let tar_name = format!("arli_snapshot_{}.tar.gz", timestamp);
        let tar_path = temp_dir.join(&tar_name);

        let output = Command::new("tar")
            .args(["-czf", tar_path.to_str().unwrap_or("snapshot.tar.gz"), "."])
            .current_dir(path)
            .output()
            .map_err(|e| format!("tar create: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("tar create failed: {}", stderr));
        }

        Ok(tar_path)
    }

    fn rollback_tar(&self, tar_path: &Path) -> Result<(), String> {
        // Clean workspace first (keep .git if present)
        if self.path.join(".git").exists() {
            let _ = Command::new("git")
                .args(["checkout", "--", "."])
                .current_dir(&self.path)
                .output();
            let _ = Command::new("git")
                .args(["clean", "-fd"])
                .current_dir(&self.path)
                .output();
        } else {
            // Remove all files except the snapshot
            for entry in std::fs::read_dir(&self.path).map_err(|e| format!("read dir: {}", e))? {
                let entry = entry.map_err(|e| format!("dir entry: {}", e))?;
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    let _ = std::fs::remove_dir_all(&entry_path);
                } else {
                    let _ = std::fs::remove_file(&entry_path);
                }
            }
        }

        // Extract tar
        let output = Command::new("tar")
            .args(["-xzf", tar_path.to_str().unwrap_or("snapshot.tar.gz")])
            .current_dir(&self.path)
            .output()
            .map_err(|e| format!("tar extract: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("tar extract failed: {}", stderr));
        }

        Ok(())
    }
}

impl Drop for WorkspaceSnapshot {
    fn drop(&mut self) {
        // Safety net: if snapshot wasn't explicitly committed or rolled back,
        // clean up the tar file. Git stash will be orphaned but that's harmless.
        if let Some(ref tar_path) = self.tar_path {
            let _ = std::fs::remove_file(tar_path);
        }
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_rollback_restores_files() {
        let tmp = std::env::temp_dir().join("arli_test_snapshot_rollback");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Create initial file
        std::fs::write(tmp.join("important.txt"), "original content").unwrap();

        let snap = WorkspaceSnapshot::create(&tmp).unwrap();
        assert!(snap.is_valid());

        // Simulate agent breaking things
        std::fs::write(tmp.join("important.txt"), "BROKEN!").unwrap();
        std::fs::write(tmp.join("junk.txt"), "garbage").unwrap();

        // Rollback
        snap.rollback().unwrap();

        // Verify original restored
        let content = std::fs::read_to_string(tmp.join("important.txt")).unwrap();
        assert_eq!(content, "original content");
        // Junk file should be gone
        assert!(!tmp.join("junk.txt").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_snapshot_commit_keeps_changes() {
        let tmp = std::env::temp_dir().join("arli_test_snapshot_commit");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        std::fs::write(tmp.join("before.txt"), "before").unwrap();
        let snap = WorkspaceSnapshot::create(&tmp).unwrap();

        // Agent does good work
        std::fs::write(tmp.join("result.txt"), "success").unwrap();

        // Commit — keep changes
        snap.commit().unwrap();

        assert!(tmp.join("before.txt").exists());
        assert!(tmp.join("result.txt").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_snapshot_nonexistent_dir_fails() {
        let result = WorkspaceSnapshot::create(Path::new("/nonexistent/path/xyz"));
        assert!(result.is_err());
    }
}
