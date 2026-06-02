//! execute_code — sandboxed Python code execution.
//!
//! Provides the agent with the ability to run Python scripts
//! in an isolated environment with:
//! - Namespace isolation (mount, net, pid, uts, ipc)
//! - Resource limits (512MB ram, 60s CPU, 100MB files)
//! - Import safety (blocked: subprocess, ctypes, multiprocessing, signal, pty, fcntl)
//! - os.system/os.popen blocked
//! - Sensitive env vars stripped (KEY, TOKEN, SECRET, PASSWORD, AUTH, PRIVATE)
//! - Optional pip package installation via whitelist

use async_trait::async_trait;
use std::time::Instant;

use super::{Tool, ToolOutput};
use crate::sandbox::{Sandbox, SandboxConfig};

/// Pip packages allowed for installation inside sandbox.
const PIP_WHITELIST: &[&str] = &[
    "numpy", "pandas", "scipy", "scikit-learn", "matplotlib",
    "pymupdf", "marker-pdf", "pillow",
    "requests", "httpx", "aiohttp",
    "pydantic", "jsonschema", "python-dotenv",
    "typer", "rich", "tqdm",
    "cryptography", "pycryptodome", "jwt",
    "redis", "sqlalchemy", "psycopg2-binary",
];

/// Safety preamble injected before user code.
///
/// Monkey-patches __import__ to block dangerous modules.
/// Patches os.system/os.popen to raise PermissionError.
/// Strips sensitive environment variables.
const SAFETY_PREAMBLE: &str = r#"
# === ARLI Sandbox Safety Guard ===
import builtins as _ab
_original_import = _ab.__import__

_BLOCKED = {
    'subprocess', 'ctypes', 'multiprocessing',
    'signal', 'pty', 'fcntl',
}

def _safe_import(name, *args, **kwargs):
    base = name.split('.')[0]
    if base in _BLOCKED:
        raise ImportError("Module '%s' is blocked by ARLI sandbox" % name)
    return _original_import(name, *args, **kwargs)

_ab.__import__ = _safe_import

# Block os.system / os.popen
import os as _os
_orig_system = getattr(_os, 'system', None)
_orig_popen = getattr(_os, 'popen', None)
if _orig_system:
    def _blocked_system(*a, **k):
        raise PermissionError("os.system() is blocked by ARLI sandbox")
    _os.system = _blocked_system
if _orig_popen:
    def _blocked_popen(*a, **k):
        raise PermissionError("os.popen() is blocked by ARLI sandbox")
    _os.popen = _blocked_popen

# Remove sensitive env vars
for _k in list(_os.environ.keys()):
    _ku = _k.upper()
    if any(p in _ku for p in ('KEY', 'TOKEN', 'SECRET', 'PASSWORD', 'AUTH', 'PRIVATE')):
        _os.environ.pop(_k, None)

del _ab, _os, _orig_system, _orig_popen, _k, _ku
# _BLOCKED and _original_import intentionally kept — referenced by _safe_import via __builtins__
# === End Safety Guard ===

"#;

pub struct ExecuteCodeTool;

#[async_trait]
impl Tool for ExecuteCodeTool {
    fn name(&self) -> &str { "execute_code" }

    fn description(&self) -> &str {
        "Execute a Python script in a sandboxed environment. \
         Full stdlib except blocked modules (subprocess, ctypes, \
         multiprocessing, signal, pty, fcntl). os.system/os.popen blocked. \
         Optional: install pip packages via 'packages' field (whitelist only). \
         Output capped at ~50KB. Timeout: 60s."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Python code to execute"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Max execution time in seconds (default: 60)",
                    "default": 60
                },
                "packages": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Pip packages to install before execution (whitelist only)"
                }
            },
            "required": ["code"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutput {
                success: false, content: String::new(),
                error: Some(format!("Invalid JSON: {}", e)),
            },
        };

        let code = match args["code"].as_str() {
            Some(c) => c,
            None => return ToolOutput {
                success: false, content: String::new(),
                error: Some("Missing required parameter: code".into()),
            },
        };

        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(60);

        let packages: Vec<String> = args.get("packages")
            .and_then(|p| p.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        for pkg in &packages {
            if !PIP_WHITELIST.contains(&pkg.as_str()) {
                return ToolOutput {
                    success: false, content: String::new(),
                    error: Some(format!(
                        "Package '{}' is not in the sandbox whitelist. Allowed: {}",
                        pkg, PIP_WHITELIST.join(", ")
                    )),
                };
            }
        }

        let start = Instant::now();
        let result = Self::run_python(code, &packages, timeout_secs);
        let elapsed_ms = start.elapsed().as_millis();

        match result {
            Ok(mut stdout) => {
                let max_len = 50 * 1024;
                if stdout.len() > max_len {
                    stdout.truncate(max_len);
                    stdout.push_str("\n\n[Output truncated at 50KB]");
                }
                ToolOutput {
                    success: true,
                    content: format!("Code executed in {}ms. Output:\n\n{}", elapsed_ms, stdout),
                    error: None,
                }
            }
            Err(e) => ToolOutput {
                success: false, content: String::new(),
                error: Some(e),
            },
        }
    }
}

impl ExecuteCodeTool {
    fn run_python(code: &str, packages: &[String], timeout_secs: u64) -> Result<String, String> {
        let script = if packages.is_empty() {
            format!("{SAFETY_PREAMBLE}\n{code}")
        } else {
            let pip_installs: String = packages.iter().map(|p| {
                format!(
                    "import subprocess as _sp\n\
                     _sp.run(['pip', 'install', '--quiet', '--target', '/tmp/pypkgs', '{}'], check=False)\n\
                     import sys as _s\n_s.path.insert(0, '/tmp/pypkgs')\n",
                    p
                )
            }).collect();
            format!("{SAFETY_PREAMBLE}\n{pip_installs}\n{code}")
        };

        let escaped = script.replace('\\', "\\\\").replace('\'', "'\\''");
        let command = format!("{} -c '{}'", "python3", escaped);

        let config = SandboxConfig {
            timeout_secs,
            memory_limit_bytes: 512 * 1024 * 1024,
            cpu_time_limit_secs: 60,
            max_file_size_bytes: 100 * 1024 * 1024,
            net_ns: true,
            allow_network: !packages.is_empty(),
            mount_ns: true,
            pid_ns: true,
            uts_ns: true,
            ipc_ns: true,
            env_passthrough: vec!["PATH".into(), "HOME".into()],
            ..SandboxConfig::default()
        };

        let output = Sandbox::execute(&command, &config);
        let clean_stderr = Self::clean_error(&output.stderr);

        if !output.stderr.is_empty() && !output.success {
            return Err(format!("Python error (exit code {}):\n{}", output.exit_code, clean_stderr));
        }
        if !output.success {
            return Err(format!(
                "Sandbox failed (exit code {}).\nstdout: {}\nstderr: {}",
                output.exit_code, output.stdout, clean_stderr
            ));
        }
        if output.killed_by_timeout {
            return Err(format!("Timeout after {}s. Partial:\n{}", timeout_secs, output.stdout));
        }

        let result = if output.stdout.is_empty() && !output.stderr.is_empty() {
            clean_stderr
        } else if !output.stdout.is_empty() {
            output.stdout
        } else {
            String::from("(no output)")
        };

        Ok(result)
    }

    fn clean_error(stderr: &str) -> String {
        stderr.lines()
            .filter(|l| !l.contains("ARLI Sandbox Safety Guard")
                && !l.contains("_safe_import")
                && !l.contains("_blocked_"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_simple_print() {
        let tool = ExecuteCodeTool;
        let result = tool.execute(
            &serde_json::json!({"code": "print('hello from sandbox')"}).to_string()
        ).await;
        assert!(result.success, "Error: {:?}", result.error);
        assert!(result.content.contains("hello from sandbox"));
    }

    #[tokio::test]
    async fn test_arithmetic() {
        let tool = ExecuteCodeTool;
        let result = tool.execute(
            &serde_json::json!({"code": "print(2 + 2)"}).to_string()
        ).await;
        assert!(result.success);
        assert!(result.content.contains('4'));
    }

    #[tokio::test]
    async fn test_syntax_error() {
        let tool = ExecuteCodeTool;
        let result = tool.execute(
            &serde_json::json!({"code": "print("}).to_string()
        ).await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_multiline() {
        let tool = ExecuteCodeTool;
        let result = tool.execute(
            &serde_json::json!({"code": "import json\ndata = {'key': 'value'}\nprint(json.dumps(data))"}).to_string()
        ).await;
        assert!(result.success);
        assert!(result.content.contains("key"));
    }

    #[tokio::test]
    async fn test_blocked_subprocess() {
        let tool = ExecuteCodeTool;
        let result = tool.execute(
            &serde_json::json!({"code": "import subprocess\nprint('NO')"}).to_string()
        ).await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_blocked_os_system() {
        let tool = ExecuteCodeTool;
        let result = tool.execute(
            &serde_json::json!({"code": "import os\nos.system('echo hacked')"}).to_string()
        ).await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_blocked_ctypes() {
        let tool = ExecuteCodeTool;
        let result = tool.execute(
            &serde_json::json!({"code": "import ctypes\nprint('NO')"}).to_string()
        ).await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_os_import_still_works() {
        let tool = ExecuteCodeTool;
        let result = tool.execute(
            &serde_json::json!({"code": "import os\nprint(os.getcwd())"}).to_string()
        ).await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_package_whitelist_blocked() {
        let tool = ExecuteCodeTool;
        let result = tool.execute(
            &serde_json::json!({"code": "print('ok')", "packages": ["scapy"]}).to_string()
        ).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("whitelist"));
    }
}
