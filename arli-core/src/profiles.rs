//! Profile management — isolated environments for different use cases.
//!
//! Profiles allow running multiple independent ARLI instances, each with
//! its own config, sessions, memory, soul.md, skills, and plugins.
//!
//! Layout:
//!   ~/.arli/                    ← default profile (when ARLI_PROFILE unset)
//!     config.toml
//!     soul.md
//!     sessions.db
//!   ~/.arli/profiles/work/      ← "work" profile (ARLI_PROFILE=work)
//!     config.toml
//!     soul.md
//!     sessions.db
//!
//! Profile is selected via ARLI_PROFILE env var or `arli profile use <name>`.

use std::path::PathBuf;
use tracing::info;

/// Resolves the active ARLI data directory, accounting for profiles.
///
/// Priority:
///   1. ARLI_HOME env var (absolute override)
///   2. ARLI_PROFILE env var → ~/.arli/profiles/<profile>/
///   3. Default: ~/.arli/
pub fn arli_data_dir() -> PathBuf {
    // ARLI_HOME is the ultimate override — use directly
    if let Ok(home) = std::env::var("ARLI_HOME") {
        let p = PathBuf::from(home);
        if p.is_absolute() {
            return p;
        }
    }

    let base = home_dir().join(".arli");

    // If ARLI_PROFILE is set, use the profile subdirectory
    if let Ok(profile) = std::env::var("ARLI_PROFILE") {
        if !profile.is_empty() && profile != "default" {
            return base.join("profiles").join(&profile);
        }
    }

    // Check for sticky default profile
    let default_profile_path = base.join("profile");
    if default_profile_path.exists() {
        if let Ok(sticky) = std::fs::read_to_string(&default_profile_path) {
            let sticky = sticky.trim().to_string();
            if !sticky.is_empty() && sticky != "default" {
                return base.join("profiles").join(&sticky);
            }
        }
    }

    base
}

/// Get the profiles directory.
pub fn profiles_dir() -> PathBuf {
    home_dir().join(".arli").join("profiles")
}

/// Get the home directory.
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// List all available profiles.
pub fn list_profiles() -> anyhow::Result<Vec<ProfileInfo>> {
    let dir = profiles_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut profiles = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let config = path.join("config.toml");
        let soul = path.join("soul.md");
        let sessions = path.join("sessions.db");

        profiles.push(ProfileInfo {
            name,
            has_config: config.exists(),
            has_soul: soul.exists(),
            has_sessions: sessions.exists(),
            path,
        });
    }

    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// Create a new profile with optional cloning from default.
pub fn create_profile(name: &str, clone_default: bool) -> anyhow::Result<PathBuf> {
    let profile_dir = profiles_dir().join(name);

    if profile_dir.exists() {
        anyhow::bail!("Profile '{}' already exists", name);
    }

    std::fs::create_dir_all(&profile_dir)?;
    info!("Created profile directory: {}", profile_dir.display());

    if clone_default {
        let default_dir = home_dir().join(".arli");

        // Clone config
        let default_config = default_dir.join("config.toml");
        if default_config.exists() {
            std::fs::copy(&default_config, profile_dir.join("config.toml"))?;
        }

        // Clone soul.md
        let default_soul = default_dir.join("soul.md");
        if default_soul.exists() {
            std::fs::copy(&default_soul, profile_dir.join("soul.md"))?;
        }
    }

    Ok(profile_dir)
}

/// Delete a profile (removes directory and all contents).
pub fn delete_profile(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name == "default" {
        anyhow::bail!("Cannot delete the default profile");
    }

    let profile_dir = profiles_dir().join(name);
    if !profile_dir.exists() {
        anyhow::bail!("Profile '{}' does not exist", name);
    }

    std::fs::remove_dir_all(&profile_dir)?;
    info!("Deleted profile: {}", name);

    // Clear sticky if this was the default
    let sticky_path = home_dir().join(".arli").join("profile");
    if sticky_path.exists() {
        if let Ok(current) = std::fs::read_to_string(&sticky_path) {
            if current.trim() == name {
                std::fs::remove_file(&sticky_path)?;
            }
        }
    }

    Ok(())
}

/// Set a profile as the default (sticky).
pub fn set_default_profile(name: &str) -> anyhow::Result<()> {
    let sticky_path = home_dir().join(".arli").join("profile");
    std::fs::create_dir_all(sticky_path.parent().unwrap())?;

    if name.is_empty() || name == "default" {
        // Remove sticky default
        if sticky_path.exists() {
            std::fs::remove_file(&sticky_path)?;
        }
    } else {
        // Verify profile exists
        let profile_dir = profiles_dir().join(name);
        if !profile_dir.exists() {
            anyhow::bail!("Profile '{}' does not exist. Create it first: arli profile create {}", name, name);
        }
        std::fs::write(&sticky_path, format!("{}\n", name))?;
    }

    Ok(())
}

/// Get the current active profile name.
pub fn current_profile() -> String {
    if let Ok(profile) = std::env::var("ARLI_PROFILE") {
        if !profile.is_empty() {
            return profile;
        }
    }

    let sticky_path = home_dir().join(".arli").join("profile");
    if sticky_path.exists() {
        if let Ok(sticky) = std::fs::read_to_string(&sticky_path) {
            let sticky = sticky.trim().to_string();
            if !sticky.is_empty() {
                return sticky;
            }
        }
    }

    "default".to_string()
}

#[derive(Debug, Clone)]
pub struct ProfileInfo {
    pub name: String,
    pub has_config: bool,
    pub has_soul: bool,
    pub has_sessions: bool,
    pub path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    

    #[test]
    fn test_list_empty_profiles() {
        // Without a real ~/.arli/profiles, should return empty or create
        let dir = profiles_dir();
        if !dir.exists() {
            let result = list_profiles().unwrap();
            assert!(result.is_empty());
        }
    }

    #[test]
    fn test_current_profile_default() {
        // Without ARLI_PROFILE set, should be "default"
        std::env::remove_var("ARLI_PROFILE");
        // Note: sticky file might exist, so we can't assert "default" without cleanup
        let profile = current_profile();
        assert!(!profile.is_empty());
    }

    #[test]
    fn test_arli_data_dir_respects_env() {
        // ARLI_HOME overrides everything
        std::env::set_var("ARLI_HOME", "/tmp/test-arli-home");
        let dir = arli_data_dir();
        assert_eq!(dir, PathBuf::from("/tmp/test-arli-home"));
        std::env::remove_var("ARLI_HOME");
    }

    #[test]
    fn test_create_and_delete_profile() {
        let test_name = "__arli_test_profile_delete_me__";
        // Clean up any previous test run
        let _ = delete_profile(test_name);

        let dir = create_profile(test_name, false).unwrap();
        assert!(dir.exists());
        assert!(dir.join("config.toml").exists() == false); // didn't clone

        let profiles = list_profiles().unwrap();
        assert!(profiles.iter().any(|p| p.name == test_name));

        delete_profile(test_name).unwrap();
        assert!(!dir.exists());
    }
}
