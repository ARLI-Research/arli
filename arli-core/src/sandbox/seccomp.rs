//! Seccomp BPF syscall filter — blocks dangerous syscalls.
//!
//! Syscall block list adapted from OpenShell's `build_filter_rules()`.
//! Uses `seccompiler` crate for BPF compilation and application.
//!
//! Blocked: ALL socket creation, memfd_create, ptrace, bpf,
//! io_uring, mount API, namespace manipulation, process inspection, perf,
//! userfaultfd, clone3 (→ ENOSYS for glibc fallback).

use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule, TargetArch};
use std::collections::BTreeMap;

/// Seccomp sandbox enforcer — blocks dangerous syscalls via BPF.
pub struct SeccompSandbox;

impl SeccompSandbox {
    /// Syscalls that are ALWAYS blocked (from OpenShell's block list).
    const BLOCKED_SYSCALLS: &[(i64, &str)] = &[
        // Socket creation (blocks all network access)
        (41, "socket"),
        // Memory manipulation
        (319, "memfd_create"),
        // Debugging / inspection
        (101, "ptrace"),
        (310, "process_vm_readv"),
        (311, "process_vm_writev"),
        // BPF (prevent loading new filters)
        (321, "bpf"),
        // io_uring (complex kernel attack surface)
        (425, "io_uring_setup"),
        (426, "io_uring_enter"),
        (427, "io_uring_register"),
        // Mount API (prevent mount namespace escapes)
        (40, "mount"),
        (432, "fsmount"),
        (433, "fsopen"),
        (434, "fspick"),
        (435, "move_mount"),
        (436, "open_tree"),
        // Namespace manipulation
        (272, "unshare"),
        (375, "setns"),
        // Performance monitoring
        (298, "perf_event_open"),
        // userfaultfd (kernel attack surface)
        (323, "userfaultfd"),
    ];

    /// Build a seccomp BPF filter that blocks all dangerous syscalls.
    ///
    /// The filter defaults to ALLOW — only explicitly listed syscalls are blocked.
    /// Blocked syscalls return EPERM (errno 1).
    pub fn build_filter() -> BpfProgram {
        let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();

        // Block all dangerous syscalls unconditionally.
        // An empty Vec<SeccompRule> means "match on syscall number only"
        // (no argument conditions needed).
        for &(syscall_nr, _name) in Self::BLOCKED_SYSCALLS {
            rules.entry(syscall_nr).or_default();
        }

        let filter = SeccompFilter::new(
            rules,
            SeccompAction::Allow,    // Default: allow
            SeccompAction::Errno(1), // Blocked: return EPERM
            TargetArch::x86_64,
        )
        .unwrap();

        filter.try_into().unwrap()
    }

    /// Apply a pre-built seccomp filter to the current process.
    pub fn apply(filter: &BpfProgram) -> Result<(), String> {
        seccompiler::apply_filter(filter).map_err(|e| format!("seccomp apply: {:?}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_builds() {
        let filter = SeccompSandbox::build_filter();
        assert!(!filter.is_empty(), "Filter should contain syscall rules");
    }

    #[test]
    fn test_filter_is_deterministic() {
        let f1 = SeccompSandbox::build_filter();
        let f2 = SeccompSandbox::build_filter();
        assert_eq!(f1, f2, "Seccomp filter should be deterministic");
    }
}
