//! Credential pools — manage multiple API keys with rotation.
//!
//! When an API key hits rate limits or errors, the pool rotates to the next.
//! Credentials are stored in `~/.arli/credentials.toml`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// A single credential entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub key: String,
    /// Optional label (e.g., "primary", "fallback", "high-priority")
    #[serde(default)]
    pub label: String,
    /// Number of times this credential has been used successfully
    #[serde(default)]
    pub success_count: u64,
    /// Number of consecutive failures
    #[serde(default)]
    pub failure_count: u64,
    /// Whether this credential is currently exhausted
    #[serde(default)]
    pub exhausted: bool,
}

/// A pool of credentials for a single provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialPool {
    pub provider: String,
    pub credentials: Vec<Credential>,
    /// Index of the currently active credential
    #[serde(default)]
    pub active_index: u64,
}

/// Credential pool manager.
pub struct CredentialManager {
    pools: Arc<Mutex<Vec<CredentialPool>>>,
    config_path: PathBuf,
}

impl CredentialManager {
    pub fn new(config_path: PathBuf) -> anyhow::Result<Self> {
        let pools = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Vec::new()
        };

        Ok(Self {
            pools: Arc::new(Mutex::new(pools)),
            config_path,
        })
    }

    /// Add a credential to a provider's pool.
    pub fn add(&self, provider: &str, key: String, label: String) -> anyhow::Result<()> {
        {
            let mut pools = self.pools.lock().unwrap();
            let pool = pools.iter_mut().find(|p| p.provider == provider);
            match pool {
                Some(p) => {
                    p.credentials.push(Credential {
                        key,
                        label,
                        success_count: 0,
                        failure_count: 0,
                        exhausted: false,
                    });
                }
                None => {
                    pools.push(CredentialPool {
                        provider: provider.to_string(),
                        credentials: vec![Credential {
                            key,
                            label,
                            success_count: 0,
                            failure_count: 0,
                            exhausted: false,
                        }],
                        active_index: 0,
                    });
                }
            }
        } // drop lock before save

        self.save()?;
        info!("Added credential for {}", provider);
        Ok(())
    }

    /// Get the active credential for a provider.
    pub fn get_active(&self, provider: &str) -> Option<String> {
        let pools = self.pools.lock().unwrap();
        let pool = pools.iter().find(|p| p.provider == provider)?;
        let idx = pool.active_index as usize;
        pool.credentials.get(idx).map(|c| c.key.clone())
    }

    /// Rotate to the next credential (on failure/rate limit).
    pub fn rotate(&self, provider: &str) -> bool {
        let mut pools = self.pools.lock().unwrap();
        let pool = match pools.iter_mut().find(|p| p.provider == provider) {
            Some(p) => p,
            None => return false,
        };

        // Mark current as exhausted
        if let Some(current) = pool.credentials.get_mut(pool.active_index as usize) {
            current.exhausted = true;
            current.failure_count += 1;
        }

        // Rotate to next non-exhausted credential
        let len = pool.credentials.len() as u64;
        for _ in 0..pool.credentials.len() {
            pool.active_index = (pool.active_index + 1) % len;
            if let Some(next) = pool.credentials.get(pool.active_index as usize) {
                if !next.exhausted {
                    info!("Rotated to credential #{} for {}", pool.active_index, provider);
                    return true;
                }
            }
        }

        // All exhausted — reset and retry from start
        warn!("All credentials for {} exhausted — resetting", provider);
        for c in &mut pool.credentials {
            c.exhausted = false;
        }
        pool.active_index = 0;
        true
    }

    /// Record a successful API call (resets failure counter).
    pub fn record_success(&self, provider: &str) {
        let mut pools = self.pools.lock().unwrap();
        if let Some(pool) = pools.iter_mut().find(|p| p.provider == provider) {
            if let Some(active) = pool.credentials.get_mut(pool.active_index as usize) {
                active.success_count += 1;
                active.failure_count = 0;
            }
        }
    }

    /// Record a failure (may trigger rotation).
    pub fn record_failure(&self, provider: &str) -> bool {
        let mut pools = self.pools.lock().unwrap();
        if let Some(pool) = pools.iter_mut().find(|p| p.provider == provider) {
            if let Some(active) = pool.credentials.get_mut(pool.active_index as usize) {
                active.failure_count += 1;
            }
            let idx = pool.active_index as usize;
            // Rotate if too many failures
            if pool.credentials[idx].failure_count >= 3 {
                drop(pools);
                return self.rotate(provider);
            }
        }
        false
    }

    /// List all pools and their credentials.
    pub fn list(&self) -> Vec<CredentialPool> {
        self.pools.lock().unwrap().clone()
    }

    /// Remove a credential by index from a provider's pool.
    pub fn remove(&self, provider: &str, index: usize) -> anyhow::Result<()> {
        {
            let mut pools = self.pools.lock().unwrap();
            let pool = pools
                .iter_mut()
                .find(|p| p.provider == provider)
                .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", provider))?;

            if index >= pool.credentials.len() {
                anyhow::bail!(
                    "Index {} out of range (0..{})",
                    index,
                    pool.credentials.len()
                );
            }

            pool.credentials.remove(index);
            if pool.active_index as usize >= pool.credentials.len() {
                pool.active_index = 0;
            }
            if pool.credentials.is_empty() {
                pools.retain(|p| p.provider != provider);
            }
        } // drop lock

        self.save()?;
        info!("Removed credential #{} for {}", index, provider);
        Ok(())
    }

    fn save(&self) -> anyhow::Result<()> {
        let pools = self.pools.lock().unwrap();
        let content = serde_json::to_string_pretty(&*pools)?;
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.config_path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_active() {
        let path = std::env::temp_dir().join("arli-test-creds.toml");
        let _ = std::fs::remove_file(&path);

        let cm = CredentialManager::new(path.clone()).unwrap();
        cm.add("openai", "sk-key1".to_string(), "primary".to_string()).unwrap();
        cm.add("openai", "sk-key2".to_string(), "fallback".to_string()).unwrap();

        assert_eq!(cm.get_active("openai"), Some("sk-key1".to_string()));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_rotate() {
        let path = std::env::temp_dir().join("arli-test-creds-rotate.toml");
        let _ = std::fs::remove_file(&path);

        let cm = CredentialManager::new(path.clone()).unwrap();
        cm.add("openai", "sk-key1".to_string(), "p1".to_string()).unwrap();
        cm.add("openai", "sk-key2".to_string(), "p2".to_string()).unwrap();

        assert!(cm.rotate("openai"));
        assert_eq!(cm.get_active("openai"), Some("sk-key2".to_string()));

        let pools = cm.list();
        let pool = pools.iter().find(|p| p.provider == "openai").unwrap();
        assert!(pool.credentials[0].exhausted);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_record_success() {
        let path = std::env::temp_dir().join("arli-test-creds-success.toml");
        let _ = std::fs::remove_file(&path);

        let cm = CredentialManager::new(path.clone()).unwrap();
        cm.add("test", "key".to_string(), "".to_string()).unwrap();
        cm.record_success("test");
        cm.record_success("test");

        let pools = cm.list();
        let c = &pools[0].credentials[0];
        assert_eq!(c.success_count, 2);
        assert_eq!(c.failure_count, 0);

        let _ = std::fs::remove_file(&path);
    }
}
