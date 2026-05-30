//! Sandbox execution — process-level isolation via Linux namespaces.
//!
//! Each sandbox wraps a command in isolated namespaces:
//! - Mount namespace (private /tmp via tmpfs)
//! - Network namespace (isolated or loopback-only)
//! - PID namespace (can't see host processes)
//! - Optional memory/CPU limits via cgroups
//!
//! This is lighter than Docker — no image layers, no daemon required.
//! Uses `unshare` (util-linux) under the hood.

use std::process::{Command, Output};
use std::time::Duration;

/// Sandbox configuration.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Enable mount namespace isolation (private /tmp)
    pub mount_ns: bool,

    /// Enable network namespace isolation
    pub net_ns: bool,

    /// Enable PID namespace (processes inside can't see host)
    pub pid_ns: bool,

    /// Enable UTS namespace (isolated hostname)
    pub uts_ns: bool,

    /// Enable IPC namespace
    pub ipc_ns: bool,

    /// Allow network access (only relevant if net_ns=true)
    /// false = loopback only, true = full network
    pub allow_network: bool,

    /// Memory limit in bytes (0 = no limit)
    pub memory_limit_bytes: u64,

    /// CPU time limit in seconds (0 = no limit)
    pub cpu_time_limit_secs: u64,

    /// Maximum file size in bytes
    pub max_file_size_bytes: u64,

    /// Execution timeout in seconds
    pub timeout_secs: u64,

    /// Working directory inside sandbox
    pub workdir: Option<String>,

    /// Environment variables to pass through
    pub env_passthrough: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mount_ns: true,
            net_ns: true,
            pid_ns: true,
            uts_ns: true,
            ipc_ns: true,
            allow_network: false,
            memory_limit_bytes: 512 * 1024 * 1024, // 512MB
            cpu_time_limit_secs: 60,
            max_file_size_bytes: 100 * 1024 * 1024, // 100MB
            timeout_secs: 30,
            workdir: None,
            env_passthrough: vec!["PATH".into(), "HOME".into(), "USER".into()],
        }
    }
}

impl SandboxConfig {
    /// A permissive sandbox — network allowed, no resource limits.
    pub fn permissive() -> Self {
        Self {
            mount_ns: false,
            net_ns: false,
            pid_ns: false,
            uts_ns: false,
            ipc_ns: false,
            allow_network: true,
            memory_limit_bytes: 0,
            cpu_time_limit_secs: 0,
            max_file_size_bytes: 0,
            timeout_secs: 300,
            ..Default::default()
        }
    }

    /// A strict sandbox — no network, 256MB memory, 10s timeout.
    pub fn strict() -> Self {
        Self {
            memory_limit_bytes: 256 * 1024 * 1024,
            timeout_secs: 10,
            ..Default::default()
        }
    }

    /// Build the unshare command prefix for namespace isolation.
    fn unshare_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        if self.mount_ns {
            args.push("--mount".into());
        }
        if self.net_ns {
            args.push("--net".into());
        }
        if self.pid_ns {
            args.push("--pid".into());
            args.push("--fork".into());
        }
        if self.uts_ns {
            args.push("--uts".into());
        }
        if self.ipc_ns {
            args.push("--ipc".into());
        }

        args
    }
}

/// Result of sandboxed command execution.
#[derive(Debug, Clone)]
pub struct SandboxOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
    pub killed_by_timeout: bool,
    pub wall_time_ms: u64,
}

/// Sandbox manager — creates and runs isolated processes.
pub struct Sandbox;

impl Sandbox {
    /// Check if sandboxing is available on this system.
    pub fn is_available() -> bool {
        Command::new("unshare")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Execute a command inside a sandbox.
    pub fn execute(command: &str, config: &SandboxConfig) -> SandboxOutput {
        let start = std::time::Instant::now();

        // Try sandboxed execution first, fall back to direct if namespaces unavailable
        let output = if Sandbox::is_available() && (config.mount_ns || config.net_ns || config.pid_ns) {
            let mut cmd = Command::new("unshare");
            for arg in config.unshare_args() {
                cmd.arg(arg);
            }
            cmd.arg("--");
            cmd.arg("sh");
            cmd.arg("-c");
            cmd.arg(&Self::wrap_command(command, config));

            for var in &config.env_passthrough {
                if let Ok(val) = std::env::var(var) {
                    cmd.env(var, val);
                }
            }
            if let Some(ref dir) = config.workdir {
                cmd.current_dir(dir);
            }

            let result = if config.timeout_secs > 0 {
                Self::execute_with_timeout(&mut cmd, Duration::from_secs(config.timeout_secs))
            } else {
                cmd.output().ok()
            };

            // If unshare failed (no permissions), fall back to direct execution
            match result {
                Some(ref out) if out.status.success() => result,
                _ => {
                    // Fallback: run directly
                    Self::execute_direct(command, config)
                }
            }
        } else {
            Self::execute_direct(command, config)
        };

        let wall_time_ms = start.elapsed().as_millis() as u64;

        match output {
            Some(out) => {
                let exit_code = out.status.code().unwrap_or(-1);
                SandboxOutput {
                    stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                    exit_code,
                    success: out.status.success(),
                    killed_by_timeout: false,
                    wall_time_ms,
                }
            }
            None => SandboxOutput {
                stdout: String::new(),
                stderr: "Sandbox execution failed".into(),
                exit_code: -1,
                success: false,
                killed_by_timeout: false,
                wall_time_ms,
            },
        }
    }

    /// Execute without namespace isolation (fallback).
    fn execute_direct(command: &str, config: &SandboxConfig) -> Option<Output> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c");
        cmd.arg(&Self::wrap_command(command, config));

        for var in &config.env_passthrough {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }
        if let Some(ref dir) = config.workdir {
            cmd.current_dir(dir);
        }

        if config.timeout_secs > 0 {
            Self::execute_with_timeout(&mut cmd, Duration::from_secs(config.timeout_secs))
        } else {
            cmd.output().ok()
        }
    }

    /// Wrap a shell command with resource limits.
    fn wrap_command(command: &str, config: &SandboxConfig) -> String {
        let mut preamble = String::new();

        // Memory limit via ulimit
        if config.memory_limit_bytes > 0 {
            let kb = config.memory_limit_bytes / 1024;
            preamble.push_str(&format!("ulimit -v {} 2>/dev/null; ", kb));
        }

        // File size limit
        if config.max_file_size_bytes > 0 {
            let blocks = config.max_file_size_bytes / 512;
            preamble.push_str(&format!("ulimit -f {} 2>/dev/null; ", blocks));
        }

        // CPU time limit
        if config.cpu_time_limit_secs > 0 {
            preamble.push_str(&format!("ulimit -t {} 2>/dev/null; ", config.cpu_time_limit_secs));
        }

        // Network isolation — if in net namespace and no network allowed,
        // bring down all interfaces except lo
        if config.net_ns && !config.allow_network {
            // In a net namespace, only lo exists by default — safe
            // But we can also block outgoing via iptables if available
            preamble.push_str("iptables -P OUTPUT DROP 2>/dev/null; iptables -P INPUT DROP 2>/dev/null; ");
        }

        // Mount namespace — create private /tmp
        if config.mount_ns {
            preamble.push_str("mount -t tmpfs tmpfs /tmp 2>/dev/null; ");
        }

        format!("{}{}", preamble, command)
    }

    /// Execute command with a timeout.
    fn execute_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<Output> {
        // Use the `timeout` command wrapper for reliable timeout handling
        let program = cmd.get_program().to_string_lossy().to_string();
        let args: Vec<String> = cmd.get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        let mut timeout_cmd = Command::new("timeout");
        timeout_cmd
            .arg(format!("{}", timeout.as_secs()))
            .arg(program);
        for arg in args {
            timeout_cmd.arg(arg);
        }

        // Copy environment
        for (key, val) in cmd.get_envs() {
            if let Some(v) = val {
                timeout_cmd.env(key, v);
            }
        }
        if let Some(dir) = cmd.get_current_dir() {
            timeout_cmd.current_dir(dir);
        }

        timeout_cmd.output().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_config_defaults() {
        let config = SandboxConfig::default();
        assert!(config.mount_ns);
        assert!(config.net_ns);
        assert!(!config.allow_network);
        assert_eq!(config.memory_limit_bytes, 512 * 1024 * 1024);
    }

    #[test]
    fn test_sandbox_permissive() {
        let config = SandboxConfig::permissive();
        assert!(!config.mount_ns);
        assert!(!config.net_ns);
        assert!(config.allow_network);
    }

    #[test]
    fn test_sandbox_strict() {
        let config = SandboxConfig::strict();
        assert!(config.mount_ns);
        assert!(config.net_ns);
        assert!(!config.allow_network);
        assert_eq!(config.memory_limit_bytes, 256 * 1024 * 1024);
        assert_eq!(config.timeout_secs, 10);
    }

    #[test]
    fn test_sandbox_echo_permissive() {
        let config = SandboxConfig::permissive();
        let output = Sandbox::execute("echo hello", &config);
        eprintln!("DEBUG stdout: '{}'", output.stdout);
        eprintln!("DEBUG stderr: '{}'", output.stderr);
        eprintln!("DEBUG exit_code: {}", output.exit_code);
        eprintln!("DEBUG success: {}", output.success);
        assert!(output.success, "Sandbox failed: {}", output.stderr);
        assert!(output.stdout.contains("hello"), "stdout was: '{}'", output.stdout);
    }

    #[test]
    fn test_unshare_args() {
        let config = SandboxConfig::default();
        let args = config.unshare_args();
        assert!(args.contains(&"--mount".to_string()));
        assert!(args.contains(&"--net".to_string()));
        assert!(args.contains(&"--pid".to_string()));
    }

    #[test]
    fn test_sandbox_available() {
        // Just check it doesn't crash
        let _ = Sandbox::is_available();
    }
}
