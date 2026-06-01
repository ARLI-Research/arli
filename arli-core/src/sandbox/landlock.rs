//! Landlock filesystem isolation — sandbox enforcement via Linux LSM.
//!
//! Uses the `landlock` crate (v0.4+) which wraps the Landlock syscall interface.
//! Kernel 5.13+ recommended (ABI v3). Falls back to older ABIs automatically.
//!
//! Pattern: build ruleset → create → add path rules → restrict_self()

use super::policy::SandboxPolicy;
use landlock::{
    path_beneath_rules, Access, AccessFs, CompatLevel, Compatible, HandledAccess, PathBeneath,
    Ruleset, RulesetAttr, RulesetCreated, RulesetCreatedAttr, RulesetStatus,
};
use std::path::Path;

/// Landlock sandbox enforcer.
pub struct LandlockSandbox;

impl LandlockSandbox {
    /// Check if Landlock is available on this kernel (ABI V1 or higher).
    pub fn is_available() -> bool {
        // Try to create a minimal ruleset — if it fails, Landlock is unavailable
        let ruleset = Ruleset::default()
            .handle_access(AccessFs::from_read(landlock::ABI::V1))
            .and_then(|r| r.create());
        ruleset.is_ok()
    }

    /// Enforce Landlock filesystem restrictions based on the given policy.
    ///
    /// Uses best-effort or hard-requirement mode based on policy config.
    /// Best-effort: silently succeed if Landlock is unavailable or partially applied.
    /// Hard-requirement: fail if Landlock is not fully enforced.
    pub fn enforce(policy: &SandboxPolicy) -> Result<(), String> {
        let compat = match policy.landlock.compatibility {
            super::policy::LandlockCompatibility::BestEffort => CompatLevel::BestEffort,
            super::policy::LandlockCompatibility::HardRequirement => CompatLevel::HardRequirement,
        };

        // Build access flags
        let ro_access = AccessFs::from_read(landlock::ABI::V1);
        let rw_access = AccessFs::from_read(landlock::ABI::V1) | AccessFs::from_write(landlock::ABI::V1);

        // Create ruleset
        let ruleset = Ruleset::default()
            .set_compatibility(compat)
            .handle_access(rw_access)
            .map_err(|e| format!("Landlock handle_access: {:?}", e))?;

        let mut created: RulesetCreated = ruleset
            .create()
            .map_err(|e| format!("Landlock create: {:?}", e))?;

        // Add read-write paths
        for path_str in &policy.filesystem.read_write {
            if let Err(e) = Self::add_path(&mut created, path_str, rw_access) {
                if compat == CompatLevel::HardRequirement {
                    return Err(format!("Landlock RW path '{}': {:?}", path_str, e));
                }
            }
        }

        // Add read-only paths
        for path_str in &policy.filesystem.read_only {
            if let Err(e) = Self::add_path(&mut created, path_str, ro_access) {
                if compat == CompatLevel::HardRequirement {
                    return Err(format!("Landlock RO path '{}': {:?}", path_str, e));
                }
            }
        }

        // Enforce via restrict_self()
        let status = created
            .restrict_self()
            .map_err(|e| format!("Landlock restrict_self: {:?}", e))?;

        match status.ruleset {
            RulesetStatus::FullyEnforced => Ok(()),
            RulesetStatus::PartiallyEnforced => {
                if compat == CompatLevel::HardRequirement {
                    Err("Landlock: partially enforced (hard requirement)".into())
                } else {
                    Ok(())
                }
            }
            RulesetStatus::NotEnforced => {
                if compat == CompatLevel::HardRequirement {
                    Err("Landlock: not enforced (hard requirement)".into())
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Add a single path rule to the ruleset.
    fn add_path(
        created: &mut RulesetCreated,
        path_str: &str,
        access: landlock::BitFlags<AccessFs>,
    ) -> Result<(), String> {
        let path = Path::new(path_str);

        // Skip non-existent paths (they add no meaningful restriction)
        if !path.exists() {
            return Ok(());
        }

        // Use path_beneath_rules() which handles PathFd internally
        let rules: Vec<PathBeneath<landlock::PathFd>> = path_beneath_rules([path], access)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("path_beneath_rules('{}'): {:?}", path_str, e))?;

        for rule in rules {
            created
                .add_rule(rule)
                .map_err(|e| format!("add_rule('{}'): {:?}", path_str, e))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available_does_not_panic() {
        let _ = LandlockSandbox::is_available();
    }

    #[test]
    fn test_enforce_restrictive_policy() {
        let policy = SandboxPolicy::restrictive_default();
        let result = LandlockSandbox::enforce(&policy);
        // Best-effort should never return Err
        assert!(
            result.is_ok(),
            "restrictive policy enforcement failed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_enforce_hard_requirement() {
        let mut policy = SandboxPolicy::restrictive_default();
        policy.landlock.compatibility =
            super::super::policy::LandlockCompatibility::HardRequirement;

        let result = LandlockSandbox::enforce(&policy);
        if LandlockSandbox::is_available() {
            assert!(
                result.is_ok(),
                "Hard requirement failed on Landlock-capable kernel: {:?}",
                result.err()
            );
        }
        // If Landlock unavailable, hard requirement may fail — that's expected
    }

    #[test]
    fn test_enforce_permissive_policy() {
        let policy = SandboxPolicy::permissive();
        let result = LandlockSandbox::enforce(&policy);
        assert!(
            result.is_ok(),
            "Permissive enforcement failed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_add_nonexistent_path_skipped() {
        let mut policy = SandboxPolicy::restrictive_default();
        policy
            .filesystem
            .read_only
            .push("/nonexistent/path/12345".into());

        let result = LandlockSandbox::enforce(&policy);
        assert!(
            result.is_ok(),
            "Nonexistent path should be skipped: {:?}",
            result.err()
        );
    }
}
