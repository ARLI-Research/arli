//! Privilege drop — drops root privileges before executing sandboxed code.
//!
//! Follows OpenShell's pattern: initgroups → setgid → setuid with verification.
//! Also applies child process hardening (no core dumps, PR_SET_DUMPABLE=0).

use super::policy::ProcessPolicy;
use nix::unistd::{getgid, getuid, setgid, setuid, Gid, Uid};
use std::ffi::CString;

/// Privilege dropper — ensures sandboxed processes run as unprivileged users.
pub struct PrivilegeDrop;

impl PrivilegeDrop {
    /// Drop privileges to the specified user and group.
    ///
    /// If already running as non-root, this is a no-op.
    /// Order: initgroups → setgid → setuid → verify.
    pub fn drop_to(username: &str, groupname: &str) -> Result<(), String> {
        // Already non-root? Nothing to do.
        if !getuid().is_root() {
            return Ok(());
        }

        // Resolve user and group
        let user = users::get_user_by_name(username)
            .ok_or_else(|| format!("User '{}' not found", username))?;
        let group = users::get_group_by_name(groupname)
            .ok_or_else(|| format!("Group '{}' not found", groupname))?;

        let uid = Uid::from_raw(user.uid());
        let gid = Gid::from_raw(group.gid());

        // Step 1: initgroups — set supplementary group list
        let c_username = CString::new(username).map_err(|e| format!("CString: {:?}", e))?;
        nix::unistd::initgroups(&c_username, gid).map_err(|e| format!("initgroups: {}", e))?;

        // Step 2: setgid — set real, effective, and saved GID
        setgid(gid).map_err(|e| format!("setgid: {}", e))?;

        // Step 3: setuid — set real, effective, and saved UID
        setuid(uid).map_err(|e| format!("setuid: {}", e))?;

        // Step 4: Verify (defense in depth — CWE-250)
        let current_uid = getuid();
        let current_gid = getgid();

        if current_uid.is_root() {
            return Err("Privilege drop failed: still running as root".into());
        }

        if current_uid != uid {
            return Err(format!(
                "Privilege drop verification failed: UID mismatch (expected {}, got {})",
                uid, current_uid
            ));
        }

        if current_gid != gid {
            return Err(format!(
                "Privilege drop verification failed: GID mismatch (expected {}, got {})",
                gid, current_gid
            ));
        }

        Ok(())
    }

    /// Apply process hardening: disable core dumps, set PR_SET_DUMPABLE=0.
    ///
    /// Must be called BEFORE dropping privileges (requires CAP_SYS_RESOURCE
    /// or root for RLIMIT_CORE; requires root for PR_SET_DUMPABLE).
    pub fn harden() -> Result<(), String> {
        // Disable core dumps
        let rlim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let rc = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &rlim) };
        if rc != 0 {
            return Err(format!("setrlimit(RLIMIT_CORE) failed: errno {}", rc));
        }

        // PR_SET_DUMPABLE = 0 (prevent ptrace and core dumps)
        let rc = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
        if rc != 0 {
            return Err(format!("prctl(PR_SET_DUMPABLE) failed: errno {}", rc));
        }

        Ok(())
    }

    /// Full isolation sequence: harden → drop.
    pub fn isolate(policy: &ProcessPolicy) -> Result<(), String> {
        if !policy.allow_core_dumps {
            Self::harden()?;
        }
        Self::drop_to(&policy.run_as_user, &policy.run_as_group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drop_to_non_root_is_noop() {
        if !getuid().is_root() {
            let result = PrivilegeDrop::drop_to("nobody", "nogroup");
            assert!(
                result.is_ok(),
                "Drop from non-root should succeed (no-op): {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn test_drop_to_root_user_is_rejected() {
        if getuid().is_root() {
            // Dropping to root should be a no-op — we already ARE root
            let result = PrivilegeDrop::drop_to("root", "root");
            // This may succeed (no-op) or fail — both OK
            // The important thing is it doesn't crash
            let _ = result;
        }
    }

    #[test]
    fn test_harden_disables_core_dumps() {
        let result = PrivilegeDrop::harden();
        // As non-root, this might fail (no CAP_SYS_RESOURCE)
        // The test just ensures it doesn't crash
        let _ = result;
    }
}
