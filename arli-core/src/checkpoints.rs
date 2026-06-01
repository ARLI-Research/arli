//! Filesystem checkpoints — snapshot and rollback for agent file operations.
//!
//! Each time the agent modifies a file (via write/patch), the CheckpointManager
//! can take a snapshot. Rollback restores all tracked files to their snapshot state.
//!
//! Snapshots are stored in `~/.arli/checkpoints/<session_id>/<N>/`.
//! The manifest file tracks which files were snapshot and their original states.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

/// A single checkpoint — snapshot of files before a tool operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: usize,
    pub timestamp: String,
    /// Map of file path → snapshot path within checkpoint dir.
    pub files: HashMap<String, String>,
}

/// Manifest for all checkpoints in a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointManifest {
    pub session_id: String,
    pub checkpoints: Vec<Checkpoint>,
    pub next_id: usize,
}

pub struct CheckpointManager {
    base_dir: PathBuf,
    session_id: String,
    pub manifest: CheckpointManifest,
}

impl CheckpointManager {
    pub fn new(base_dir: PathBuf, session_id: String) -> anyhow::Result<Self> {
        let session_dir = base_dir.join(&session_id);
        std::fs::create_dir_all(&session_dir)?;

        let manifest_path = session_dir.join("manifest.json");
        let manifest = if manifest_path.exists() {
            let content = std::fs::read_to_string(&manifest_path)?;
            serde_json::from_str(&content).unwrap_or(CheckpointManifest {
                session_id: session_id.clone(),
                checkpoints: Vec::new(),
                next_id: 1,
            })
        } else {
            CheckpointManifest {
                session_id: session_id.clone(),
                checkpoints: Vec::new(),
                next_id: 1,
            }
        };

        Ok(Self {
            base_dir,
            session_id,
            manifest,
        })
    }

    /// Take a snapshot of a file before modification.
    /// Call this BEFORE the agent writes to the file.
    pub fn snapshot_file(&mut self, file_path: &Path) -> anyhow::Result<()> {
        if !file_path.exists() {
            // New file — nothing to snapshot
            return Ok(());
        }

        let id = self.manifest.next_id;
        let cp_dir = self.base_dir.join(&self.session_id).join(id.to_string());
        std::fs::create_dir_all(&cp_dir)?;

        let rel_path = file_path.to_string_lossy().to_string();
        let snapshot_name = sanitize_filename(&rel_path);
        let snapshot_path = cp_dir.join(&snapshot_name);

        std::fs::copy(file_path, &snapshot_path)?;

        // Add to current checkpoint or create new one
        if let Some(last) = self.manifest.checkpoints.last_mut() {
            last.files.insert(
                rel_path.clone(),
                snapshot_path.to_string_lossy().to_string(),
            );
        } else {
            let cp = Checkpoint {
                id,
                timestamp: chrono::Utc::now().to_rfc3339(),
                files: {
                    let mut m = HashMap::new();
                    m.insert(rel_path, snapshot_path.to_string_lossy().to_string());
                    m
                },
            };
            self.manifest.checkpoints.push(cp);
            self.manifest.next_id = id + 1;
        }

        self.save_manifest()?;
        info!("Snapshot: {} → checkpoint {}", file_path.display(), id);
        Ok(())
    }

    /// Create a new checkpoint group (starts a new snapshot batch).
    pub fn new_checkpoint(&mut self) -> usize {
        let id = self.manifest.next_id;
        let cp = Checkpoint {
            id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            files: HashMap::new(),
        };
        self.manifest.checkpoints.push(cp);
        self.manifest.next_id = id + 1;
        let _ = self.save_manifest();
        id
    }

    /// Rollback to the last checkpoint — restore all files.
    pub fn rollback(&self) -> anyhow::Result<usize> {
        let last = self
            .manifest
            .checkpoints
            .last()
            .ok_or_else(|| anyhow::anyhow!("No checkpoints to rollback to"))?;

        let mut restored = 0;
        for (orig_path, snapshot_path) in &last.files {
            let orig = Path::new(orig_path);
            let snap = Path::new(snapshot_path);

            if snap.exists() {
                if let Some(parent) = orig.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(snap, orig)?;
                restored += 1;
                info!("Rollback: restored {}", orig.display());
            }
        }

        Ok(restored)
    }

    /// List all checkpoints for this session.
    pub fn list(&self) -> &[Checkpoint] {
        &self.manifest.checkpoints
    }

    /// Prune old checkpoints, keeping the last N.
    pub fn prune(&mut self, keep: usize) -> anyhow::Result<usize> {
        if self.manifest.checkpoints.len() <= keep {
            return Ok(0);
        }

        let to_remove: Vec<_> = self
            .manifest
            .checkpoints
            .iter()
            .take(self.manifest.checkpoints.len() - keep)
            .map(|c| c.id)
            .collect();

        let mut removed = 0;
        for id in &to_remove {
            let cp_dir = self.base_dir.join(&self.session_id).join(id.to_string());
            if cp_dir.exists() {
                std::fs::remove_dir_all(&cp_dir)?;
            }
            removed += 1;
        }

        self.manifest.checkpoints = self
            .manifest
            .checkpoints
            .iter()
            .skip(self.manifest.checkpoints.len().saturating_sub(keep))
            .cloned()
            .collect();

        self.save_manifest()?;
        Ok(removed)
    }

    fn save_manifest(&self) -> anyhow::Result<()> {
        let manifest_path = self.base_dir.join(&self.session_id).join("manifest.json");
        let content = serde_json::to_string_pretty(&self.manifest)?;
        std::fs::write(&manifest_path, content)?;
        Ok(())
    }
}

fn sanitize_filename(path: &str) -> String {
    path.replace(['/', '\\', ':'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_and_rollback() {
        let dir = std::env::temp_dir().join("arli-test-checkpoints-1");
        let _ = std::fs::remove_dir_all(&dir);

        // Create a test file
        let test_file = dir.join("test.txt");
        std::fs::create_dir_all(test_file.parent().unwrap()).unwrap();
        std::fs::write(&test_file, "original content").unwrap();

        // Snapshot
        let mut cm = CheckpointManager::new(dir.clone(), "test-session".to_string()).unwrap();
        cm.snapshot_file(&test_file).unwrap();

        // Modify
        std::fs::write(&test_file, "modified content").unwrap();

        // Rollback
        let restored = cm.rollback().unwrap();
        assert_eq!(restored, 1);

        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "original content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prune() {
        let dir = std::env::temp_dir().join("arli-test-checkpoints-prune");
        let _ = std::fs::remove_dir_all(&dir);

        let mut cm = CheckpointManager::new(dir.clone(), "prune-test".to_string()).unwrap();

        // Create 5 checkpoints
        for _i in 0..5 {
            cm.new_checkpoint();
        }
        assert_eq!(cm.list().len(), 5);

        // Prune to 2
        let removed = cm.prune(2).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(cm.list().len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
