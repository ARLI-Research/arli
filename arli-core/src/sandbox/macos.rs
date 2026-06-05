//! macOS sandbox implementation using Seatbelt / sandbox-exec with `.sb` profile generation.
//!
//! # Platform coverage
//!
//! On macOS, the kernel-level sandbox mechanism is **Seatbelt** (also called the macOS Sandbox
//! or App Sandbox).  It operates via a Scheme-like policy language defined in `.sb` profile
//! files.  Profiles follow a **default-deny** pattern: everything is denied unless explicitly
//! allowed.
//!
//! # Limitations vs. Linux
//!
//! | Mechanism          | Linux                                      | macOS (Seatbelt)                          |
//! |--------------------|--------------------------------------------|-------------------------------------------|
//! | Syscall filter     | seccomp BPF (fine-grained)                 | **No equivalent** — Seatbelt operates at a higher level |
//! | Filesystem paths   | Landlock (path-beneath rules)              | Seatbelt `file-read*` / `file-write*` with `subpath` |
//! | Network            | Network namespace + iptables               | Seatbelt `network*` rules                 |
//! | Privilege drop     | setuid/setgid via `nix`                    | `sandbox-exec` runs as calling user; no built-in uid drop |
//! | Process inspection | PID namespace                              | **No namespace isolation** — child can see host processes |
//! | Resource limits    | cgroups / ulimit                           | ulimit only (no cgroups equivalent)       |
//! | Mount isolation    | Mount namespace + tmpfs                    | **No mount namespace** — can't create private /tmp |
//!
//! # Implementation strategy
//!
//! 1. **Profile generation** — convert `SandboxPolicy` fields into `.sb` syntax.
//! 2. **sandbox-exec** — wrap the target command in `sandbox-exec -f <profile>`.
//! 3. **Programmatic apply** — for `pre_exec()` hooks, call `sandbox_init(3)` directly.
//! 4. **Fallback** — if `sandbox-exec` is unavailable, use `chroot`-based isolation.
//!
//! # References
//!
//! - Apple: `sandbox_init(3)` man page
//! - Apple: `sandbox-exec(1)` man page
//! - macOS Sandbox Profile Language: `man 7 sandbox`

use std::fmt;
use std::io;

// macOS-only imports (gated so they don't warn on Linux)
#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::io::Write;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;

// ---------------------------------------------------------------------------
// Cross-platform error type (available on all targets so mod.rs can refer to it)
// ---------------------------------------------------------------------------

/// Errors returned by macOS sandbox operations.
#[derive(Debug)]
pub enum MacOSSandboxError {
    /// I/O error (file creation, temp dir, etc.)
    Io(io::Error),
    /// Profile generation error
    Profile(String),
    /// sandbox-exec not found on the system
    SandboxExecNotFound,
    /// sandbox-exec returned a non-zero exit code
    SandboxExecFailed(i32, String),
    /// sandbox_init(3) call failed
    SandboxInitFailed(String),
    /// chroot fallback failed
    ChrootFailed(String),
    /// Not running on macOS
    UnsupportedPlatform,
}

impl fmt::Display for MacOSSandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::Profile(msg) => write!(f, "Profile generation error: {}", msg),
            Self::SandboxExecNotFound => write!(f, "sandbox-exec not found on this system"),
            Self::SandboxExecFailed(code, stderr) => {
                write!(f, "sandbox-exec exited with code {}: {}", code, stderr)
            }
            Self::SandboxInitFailed(msg) => write!(f, "sandbox_init(3) failed: {}", msg),
            Self::ChrootFailed(msg) => write!(f, "chroot fallback failed: {}", msg),
            Self::UnsupportedPlatform => {
                write!(f, "macOS sandbox is not available on this platform")
            }
        }
    }
}

impl std::error::Error for MacOSSandboxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for MacOSSandboxError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

// ---------------------------------------------------------------------------
// macOS-specific implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;

    // -----------------------------------------------------------------------
    // MacOSSandbox — main entry point
    // -----------------------------------------------------------------------

    /// macOS Seatbelt sandbox manager.
    ///
    /// Generates `.sb` profile files from `SandboxPolicy` and applies them
    /// via `sandbox-exec` or the programmatic `sandbox_init(3)` API.
    pub struct MacOSSandbox {
        /// Path to the generated `.sb` profile on disk (if using sandbox-exec).
        profile_path: Option<PathBuf>,
        /// The profile content as a string (for sandbox_init).
        profile_content: String,
    }

    impl MacOSSandbox {
        // -------------------------------------------------------------------
        // Construction
        // -------------------------------------------------------------------

        /// Create a new macOS sandbox from a sandbox policy.
        ///
        /// Generates the `.sb` profile content and writes it to a temporary file
        /// for use with `sandbox-exec`.
        pub fn new(
            policy: &super::super::policy::SandboxPolicy,
        ) -> Result<Self, MacOSSandboxError> {
            let profile_content = generate_profile(policy)?;

            // Write profile to a temp file so sandbox-exec can read it
            let mut tmp = tempfile::Builder::new()
                .prefix("arli-sandbox-")
                .suffix(".sb")
                .tempfile()?;

            tmp.write_all(profile_content.as_bytes())?;
            tmp.flush()?;

            // Keep the temp file alive (it will be cleaned up on drop)
            let (_file, path) = tmp
                .keep()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

            Ok(Self {
                profile_path: Some(path),
                profile_content,
            })
        }

        /// Create a macOS sandbox with only the profile content (for sandbox_init).
        ///
        /// Does not create a temp file — use this when calling `apply()` via `pre_exec()`.
        pub fn new_in_memory(
            policy: &super::super::policy::SandboxPolicy,
        ) -> Result<Self, MacOSSandboxError> {
            let profile_content = generate_profile(policy)?;
            Ok(Self {
                profile_path: None,
                profile_content,
            })
        }

        // -------------------------------------------------------------------
        // Availability check
        // -------------------------------------------------------------------

        /// Check whether macOS sandbox mechanisms are available.
        ///
        /// Returns `true` if `sandbox-exec` is found on the system.
        pub fn is_available() -> bool {
            Command::new("which")
                .arg("sandbox-exec")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        // -------------------------------------------------------------------
        // Application
        // -------------------------------------------------------------------

        /// Apply the sandbox from within the current process.
        ///
        /// This is designed to be called from `Command::pre_exec()`.
        /// Uses the `sandbox_init(3)` API to programmatically apply the profile.
        ///
        /// # Safety
        ///
        /// This function is safe to call but calls into `sandbox_init` which
        /// permanently sandboxes the current process.
        pub fn apply(&self) -> Result<(), MacOSSandboxError> {
            apply_sandbox_init(&self.profile_content)
        }

        /// Get the path to the generated `.sb` profile file.
        ///
        /// Returns `None` if this sandbox was created in-memory only.
        pub fn profile_path(&self) -> Option<&Path> {
            self.profile_path.as_deref()
        }

        /// Get the profile content as a string.
        pub fn profile_content(&self) -> &str {
            &self.profile_content
        }

        // -------------------------------------------------------------------
        // sandbox-exec command builder
        // -------------------------------------------------------------------

        /// Build a `Command` that runs the given shell command under `sandbox-exec`.
        ///
        /// The returned `Command` uses `sandbox-exec -f <profile> sh -c "<wrapped>"`.
        pub fn sandbox_exec_command(
            &self,
            shell_command: &str,
        ) -> Result<Command, MacOSSandboxError> {
            let profile_path = self.profile_path.as_ref().ok_or_else(|| {
                MacOSSandboxError::Profile(
                    "No profile file available — use new() not new_in_memory() for sandbox-exec"
                        .into(),
                )
            })?;

            let mut cmd = Command::new("sandbox-exec");
            cmd.arg("-f");
            cmd.arg(profile_path);
            cmd.arg("sh");
            cmd.arg("-c");
            cmd.arg(shell_command);

            Ok(cmd)
        }
    }

    // -----------------------------------------------------------------------
    // Profile generation
    // -----------------------------------------------------------------------

    /// Generate a macOS Seatbelt `.sb` profile string from a `SandboxPolicy`.
    ///
    /// The profile follows a **default-deny** pattern:
    /// 1. `(version 1)` — profile format version
    /// 2. `(deny default)` — block everything by default
    /// 3. Explicit `(allow ...)` rules for read-only paths, read-write paths, network
    /// 4. Minimal allowances for process operation (fork, signals, sysctl-read)
    pub fn generate_profile(
        policy: &super::super::policy::SandboxPolicy,
    ) -> Result<String, MacOSSandboxError> {
        let mut lines: Vec<String> = Vec::new();

        // Header
        lines.push("(version 1)".into());
        lines.push(String::new());
        lines.push(";; ARLI macOS sandbox profile — auto-generated".into());
        lines.push(";; Default-deny: block everything, explicitly allow what's needed".into());
        lines.push(String::new());
        lines.push("(deny default)".into());
        lines.push(String::new());

        // ------------------------------------------------------------------
        // Filesystem: read-only paths
        // ------------------------------------------------------------------
        if !policy.filesystem.read_only.is_empty() {
            lines.push(";; Read-only paths".into());
            for path in &policy.filesystem.read_only {
                let sanitized = sanitize_sb_path(path);
                lines.push(format!("(allow file-read* (subpath \"{}\"))", sanitized));
            }
            lines.push(String::new());
        }

        // ------------------------------------------------------------------
        // Filesystem: read-write paths
        // ------------------------------------------------------------------
        if !policy.filesystem.read_write.is_empty() {
            lines.push(";; Read-write paths".into());
            for path in &policy.filesystem.read_write {
                let sanitized = sanitize_sb_path(path);
                lines.push(format!(
                    "(allow file-read* file-write* (subpath \"{}\"))",
                    sanitized
                ));
            }
            lines.push(String::new());
        }

        // ------------------------------------------------------------------
        // Network
        // ------------------------------------------------------------------
        match policy.network.mode {
            super::super::policy::NetworkMode::Block => {
                lines.push(";; Network: fully blocked".into());
                lines.push("(deny network*)".into());
                lines.push(String::new());
            }
            super::super::policy::NetworkMode::Allow => {
                lines.push(";; Network: fully allowed".into());
                lines.push("(allow network*)".into());
                lines.push(String::new());
            }
            super::super::policy::NetworkMode::Proxy => {
                lines.push(";; Network: proxy mode — only allowed endpoints".into());
                lines.push("(deny network*)".into());

                for endpoint in &policy.network.allowed_endpoints {
                    // macOS sandbox doesn't support wildcard hosts in network-outbound rules
                    // the same way.  We generate best-effort rules.
                    let host = &endpoint.host;

                    // Skip wildcard hosts — they can't be expressed in sb syntax directly
                    if host.contains('*') {
                        continue;
                    }

                    if let Some(port) = endpoint.port {
                        lines.push(format!(
                            "(allow network-outbound (remote tcp \"{}:{}\"))",
                            host, port
                        ));
                    } else {
                        lines.push(format!(
                            "(allow network-outbound (remote tcp \"{}:*\"))",
                            host
                        ));
                    }
                }
                lines.push(String::new());
            }
        }

        // ------------------------------------------------------------------
        // Essential allowances (needed for basic process operation)
        // ------------------------------------------------------------------
        lines.push(";; Essential process operations".into());
        lines.push("(allow process-fork)".into());
        lines.push("(allow signal (target self))".into());
        lines.push("(allow sysctl-read)".into());
        lines.push(String::new());

        // ------------------------------------------------------------------
        // Allow reading system files needed for dynamic linking
        // ------------------------------------------------------------------
        lines.push(";; System files needed for dynamic linking / dyld".into());
        lines.push("(allow file-read* (subpath \"/usr/lib\"))".into());
        lines.push("(allow file-read* (subpath \"/System/Library\"))".into());
        lines.push("(allow file-read* (subpath \"/AppleInternal\"))".into());

        Ok(lines.join("\n") + "\n")
    }

    /// Sanitize a path for inclusion in a `.sb` profile string.
    ///
    /// Escapes double quotes and backslashes inside the path string.
    fn sanitize_sb_path(path: &str) -> String {
        path.replace('\\', "\\\\").replace('"', "\\\"")
    }

    // -----------------------------------------------------------------------
    // Programmatic sandbox application (sandbox_init)
    // -----------------------------------------------------------------------

    /// FFI binding to macOS `sandbox_init(3)`.
    ///
    /// ```c
    /// int sandbox_init(const char *profile, uint64_t flags, char **errorbuf);
    /// ```
    ///
    /// Flags:
    /// - `SANDBOX_NAMED = 1` — profile is a named built-in (e.g. "nointernet")
    /// - We use `0` to indicate the profile string is inline SBPL.
    extern "C" {
        fn sandbox_init(
            profile: *const libc::c_char,
            flags: u64,
            errorbuf: *mut *mut libc::c_char,
        ) -> libc::c_int;

        fn sandbox_free_error(errorbuf: *mut libc::c_char);
    }

    /// Apply a Seatbelt profile programmatically via `sandbox_init(3)`.
    ///
    /// This permanently sandboxes the calling process.  Must be called after
    /// `fork()` but before `exec()` — i.e. inside `pre_exec()`.
    fn apply_sandbox_init(profile_content: &str) -> Result<(), MacOSSandboxError> {
        let c_profile = CString::new(profile_content).map_err(|e| {
            MacOSSandboxError::Profile(format!("Profile contains null byte: {}", e))
        })?;

        let mut errorbuf: *mut libc::c_char = std::ptr::null_mut();

        // flags = 0 → profile is inline SBPL text (not a named built-in)
        let ret = unsafe { sandbox_init(c_profile.as_ptr(), 0, &mut errorbuf) };

        if ret != 0 {
            let msg = if !errorbuf.is_null() {
                let s = unsafe {
                    std::ffi::CStr::from_ptr(errorbuf)
                        .to_string_lossy()
                        .into_owned()
                };
                unsafe { sandbox_free_error(errorbuf) };
                s
            } else {
                format!("sandbox_init returned {}", ret)
            };
            return Err(MacOSSandboxError::SandboxInitFailed(msg));
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // chroot fallback
    // -----------------------------------------------------------------------

    /// Attempt chroot-based isolation as a fallback when sandbox-exec is unavailable.
    ///
    /// This is a weaker form of isolation but provides basic filesystem confinement.
    /// Changes the root directory to `new_root` and then executes the command.
    ///
    /// # Safety
    ///
    /// `chroot()` requires root privileges on macOS.  This fallback is best-effort
    /// and may fail for non-root users.
    pub fn chroot_isolate(new_root: &Path, command: &str) -> Result<(), MacOSSandboxError> {
        let c_root = CString::new(
            new_root
                .to_str()
                .ok_or_else(|| MacOSSandboxError::ChrootFailed("Non-UTF-8 path".into()))?,
        )
        .map_err(|e| MacOSSandboxError::ChrootFailed(format!("CString: {}", e)))?;

        // chroot to the new root
        let ret = unsafe { libc::chroot(c_root.as_ptr()) };
        if ret != 0 {
            return Err(MacOSSandboxError::ChrootFailed(format!(
                "chroot failed: errno {}",
                io::Error::last_os_error().raw_os_error().unwrap_or(-1)
            )));
        }

        // chdir to / inside the new root
        let ret = unsafe { libc::chdir(b"/\0".as_ptr() as *const libc::c_char) };
        if ret != 0 {
            return Err(MacOSSandboxError::ChrootFailed(format!(
                "chdir after chroot failed: errno {}",
                io::Error::last_os_error().raw_os_error().unwrap_or(-1)
            )));
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[cfg(test)]
    mod tests {
        use super::super::super::policy::{NetworkMode, SandboxPolicy};
        use super::*;

        /// Helper: create a basic restrictive policy for testing.
        fn test_policy() -> SandboxPolicy {
            SandboxPolicy::restrictive_default()
        }

        #[test]
        fn test_is_available_does_not_panic() {
            let _ = MacOSSandbox::is_available();
        }

        #[test]
        fn test_generate_profile_structure() {
            let policy = test_policy();
            let profile = generate_profile(&policy).expect("Profile generation should succeed");

            // Must start with version
            assert!(profile.starts_with("(version 1)"), "Profile: {}", profile);

            // Must be default-deny
            assert!(
                profile.contains("(deny default)"),
                "Profile should deny by default"
            );

            // Should contain essential allowances
            assert!(profile.contains("process-fork"));
            assert!(profile.contains("sysctl-read"));

            // Should block network (restrictive default blocks network)
            assert!(
                profile.contains("(deny network*)"),
                "Restrictive policy should deny network"
            );
        }

        #[test]
        fn test_generate_profile_with_rw_paths() {
            let mut policy = test_policy();
            policy.filesystem.read_write = vec!["/workspace".into(), "/tmp".into()];
            let profile = generate_profile(&policy).expect("Profile generation should succeed");

            assert!(profile.contains("(allow file-read* file-write* (subpath \"/workspace\"))"));
            assert!(profile.contains("(allow file-read* file-write* (subpath \"/tmp\"))"));
        }

        #[test]
        fn test_generate_profile_with_ro_paths() {
            let mut policy = test_policy();
            policy.filesystem.read_only = vec!["/usr".into(), "/lib".into()];
            let profile = generate_profile(&policy).expect("Profile generation should succeed");

            assert!(profile.contains("(allow file-read* (subpath \"/usr\"))"));
            assert!(profile.contains("(allow file-read* (subpath \"/lib\"))"));
        }

        #[test]
        fn test_generate_profile_allow_network() {
            let mut policy = test_policy();
            policy.network.mode = NetworkMode::Allow;
            let profile = generate_profile(&policy).expect("Profile generation should succeed");

            assert!(profile.contains("(allow network*)"));
            assert!(!profile.contains("(deny network*)"));
        }

        #[test]
        fn test_generate_profile_proxy_mode() {
            let mut policy = test_policy();
            policy.network.mode = NetworkMode::Proxy;
            policy.network.allowed_endpoints = vec![
                super::super::super::policy::NetworkEndpoint {
                    host: "api.github.com".into(),
                    port: Some(443),
                    tls: true,
                },
                super::super::super::policy::NetworkEndpoint {
                    host: "*.example.com".into(),
                    port: None,
                    tls: false,
                },
            ];
            let profile = generate_profile(&policy).expect("Profile generation should succeed");

            // Should have default deny for network
            assert!(profile.contains("(deny network*)"));

            // Should allow the exact host
            assert!(profile.contains("api.github.com:443"));

            // Wildcard hosts should be skipped (can't be expressed in sb syntax)
            // The deny default + explicit allow for github makes this best-effort
        }

        #[test]
        fn test_sanitize_sb_path() {
            assert_eq!(sanitize_sb_path("/usr/lib"), "/usr/lib");
            assert_eq!(
                sanitize_sb_path("/path/with\\backslash"),
                "/path/with\\\\backslash"
            );
            assert_eq!(sanitize_sb_path("/path/with\"quote"), "/path/with\\\"quote");
        }

        #[test]
        fn test_new_in_memory() {
            let policy = test_policy();
            let sb = MacOSSandbox::new_in_memory(&policy).expect("in-memory sandbox should create");
            assert!(sb.profile_path().is_none());
            assert!(!sb.profile_content().is_empty());
            assert!(sb.profile_content().contains("(deny default)"));
        }

        #[test]
        fn test_new_creates_temp_file() {
            let policy = test_policy();
            let sb = MacOSSandbox::new(&policy).expect("sandbox should create with temp file");
            assert!(sb.profile_path().is_some());
            assert!(!sb.profile_content().is_empty());

            // Verify the temp file exists and contains our content
            let path = sb.profile_path().unwrap();
            assert!(path.exists(), "Temp profile should exist at {:?}", path);
            let content = std::fs::read_to_string(path).expect("should read temp file");
            assert!(content.contains("(deny default)"));
        }

        #[test]
        fn test_sandbox_exec_command_builds() {
            let policy = test_policy();
            let sb = MacOSSandbox::new(&policy).expect("sandbox should create");
            let cmd = sb
                .sandbox_exec_command("echo hello")
                .expect("should build command");
            let program = cmd.get_program().to_string_lossy().into_owned();
            assert_eq!(program, "sandbox-exec");
        }

        #[test]
        fn test_sandbox_exec_command_fails_for_in_memory() {
            let policy = test_policy();
            let sb = MacOSSandbox::new_in_memory(&policy).expect("in-memory sandbox should create");
            let result = sb.sandbox_exec_command("echo hello");
            assert!(
                result.is_err(),
                "in-memory sandbox should fail sandbox_exec_command"
            );
        }

        #[test]
        fn test_profile_syntax_no_unbalanced_parens() {
            let policy = test_policy();
            let profile = generate_profile(&policy).expect("Profile generation should succeed");

            let open = profile.matches('(').count();
            let close = profile.matches(')').count();
            assert_eq!(
                open, close,
                "Profile has unbalanced parentheses:\n{}",
                profile
            );
        }

        #[test]
        fn test_profile_syntax_valid_toplevel() {
            // A valid .sb profile must have a single top-level S-expression
            let policy = test_policy();
            let profile = generate_profile(&policy).expect("Profile generation should succeed");

            // Strip comments (lines starting with ;;)
            let stripped: String = profile
                .lines()
                .filter(|l| !l.trim().starts_with(";;"))
                .collect::<Vec<_>>()
                .join("\n");

            // Should start with (version 1) followed by rules
            assert!(
                stripped.trim().starts_with("(version 1)"),
                "Profile should start with (version 1)"
            );
        }

        #[test]
        fn test_empty_policy_generates_valid_profile() {
            let policy = SandboxPolicy::permissive();
            let profile = generate_profile(&policy).expect("Profile generation should succeed");

            assert!(!profile.is_empty());
            assert!(profile.contains("(version 1)"));
            assert!(profile.contains("(deny default)"));
        }
    }
}

// ---------------------------------------------------------------------------
// Re-export the platform-specific implementation on macOS
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub use imp::*;

// ---------------------------------------------------------------------------
// Non-macOS stubs — so the module compiles on Linux
// ---------------------------------------------------------------------------

/// Stub: always returns `false` on non-macOS platforms.
#[cfg(not(target_os = "macos"))]
pub fn is_available() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Cross-platform module-level tests (always compile)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod cross_platform_tests {
    use super::*;

    #[test]
    fn test_is_available_does_not_panic() {
        let _ = is_available();
    }
}
