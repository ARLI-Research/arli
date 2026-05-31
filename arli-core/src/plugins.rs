//! Plugin system — subprocess-based extensions with JSON-RPC protocol.
//!
//! Plugins are executables/scripts in `~/.arli/plugins/<name>/` that
//! communicate via JSON-RPC over stdin/stdout (same protocol as MCP).
//!
//! Each plugin has a `plugin.toml` manifest and an executable.
//! The plugin process is spawned on demand and tools are dispatched
//! via `tools/call` requests.

use crate::tools::{Tool, ToolOutput, ToolRegistry};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tracing::{info, warn, error};

// ── Manifest format ──

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    #[serde(default)]
    pub tools: Vec<PluginToolDef>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub executable: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_protocol() -> String {
    "json-rpc".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PluginToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

// ── Plugin process handle ──

struct PluginProcess {
    child: Child,
    plugin_name: String,
}

impl PluginProcess {
    fn send_request(&mut self, request: &str) -> anyhow::Result<String> {
        let stdin = self.child.stdin.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Plugin stdin not available"))?;
        writeln!(stdin, "{}", request)?;
        stdin.flush()?;

        let stdout = self.child.stdout.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Plugin stdout not available"))?;
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line)?;

        Ok(line)
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ── Plugin tool wrapper — delegates execute() to the plugin process ──

struct PluginToolWrapper {
    name: String,
    description: String,
    parameters: Value,
    process: Arc<Mutex<PluginProcess>>,
}

#[async_trait]
impl Tool for PluginToolWrapper {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.description }
    fn parameters_schema(&self) -> Value { self.parameters.clone() }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": self.name,
                "arguments": arguments,
            }
        });

        let mut proc = match self.process.lock() {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(&format!("Plugin lock error: {}", e)),
        };

        match proc.send_request(&request.to_string()) {
            Ok(response) => {
                match serde_json::from_str::<Value>(&response) {
                    Ok(v) => {
                        if let Some(err) = v.get("error") {
                            ToolOutput {
                                success: false,
                                content: String::new(),
                                error: Some(err.to_string()),
                            }
                        } else if let Some(result) = v.get("result") {
                            let text = result.get("content")
                                .and_then(|c| c.as_array())
                                .and_then(|arr| arr.first())
                                .and_then(|c| c.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or(&response)
                                .to_string();
                            ToolOutput {
                                success: true,
                                content: text,
                                error: None,
                            }
                        } else {
                            ToolOutput {
                                success: true,
                                content: response,
                                error: None,
                            }
                        }
                    }
                    Err(e) => ToolOutput::error(&format!("Plugin JSON parse error: {}", e)),
                }
            }
            Err(e) => ToolOutput::error(&format!("Plugin communication error: {}", e)),
        }
    }
}

// ── PluginManager ──

pub struct PluginManager {
    plugins_dir: PathBuf,
    loaded: HashMap<String, PluginInfo>,
}

struct PluginInfo {
    manifest: PluginManifest,
    process: Arc<Mutex<PluginProcess>>,
}

impl PluginManager {
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self {
            plugins_dir,
            loaded: HashMap::new(),
        }
    }

    /// Scan the plugins directory and return all discovered plugins.
    pub fn discover(&self) -> anyhow::Result<Vec<PluginManifest>> {
        let mut plugins = Vec::new();

        if !self.plugins_dir.exists() {
            return Ok(plugins);
        }

        for entry in std::fs::read_dir(&self.plugins_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("plugin.toml");
            if !manifest_path.exists() {
                continue;
            }

            match std::fs::read_to_string(&manifest_path) {
                Ok(content) => {
                    match toml::from_str::<PluginManifest>(&content) {
                        Ok(manifest) => {
                            info!("Discovered plugin: {} v{}", manifest.plugin.name, manifest.plugin.version);
                            plugins.push(manifest);
                        }
                        Err(e) => {
                            warn!("Failed to parse plugin manifest {}: {}", manifest_path.display(), e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read plugin manifest {}: {}", manifest_path.display(), e);
                }
            }
        }

        Ok(plugins)
    }

    /// Load and start a plugin. Spawns the process and performs handshake.
    pub fn load(&mut self, manifest: PluginManifest) -> anyhow::Result<()> {
        let plugin_dir = self.plugins_dir.join(&manifest.plugin.name);
        let exec_path = plugin_dir.join(&manifest.plugin.executable);

        if !exec_path.exists() {
            anyhow::bail!("Plugin executable not found: {}", exec_path.display());
        }

        info!("Loading plugin: {} ({})", manifest.plugin.name, exec_path.display());

        let mut child = Command::new(&exec_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn plugin {}: {}", manifest.plugin.name, e))?;

        let mut process = PluginProcess {
            plugin_name: manifest.plugin.name.clone(),
            child,
        };

        // Handshake
        let init_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {}
        });

        match process.send_request(&init_request.to_string()) {
            Ok(response) => {
                if let Ok(v) = serde_json::from_str::<Value>(&response) {
                    if v.get("error").is_some() {
                        let _ = process.child.kill();
                        let _ = process.child.wait();
                        anyhow::bail!("Plugin {} handshake failed: {}", manifest.plugin.name, response);
                    }
                    info!("Plugin {} handshake OK", manifest.plugin.name);
                }
            }
            Err(e) => {
                let _ = process.child.kill();
                let _ = process.child.wait();
                anyhow::bail!("Plugin {} handshake error: {}", manifest.plugin.name, e);
            }
        }

        let process = Arc::new(Mutex::new(process));
        let name = manifest.plugin.name.clone();

        self.loaded.insert(name, PluginInfo {
            manifest,
            process,
        });

        Ok(())
    }

    /// Register all loaded plugin tools into a ToolRegistry.
    pub fn register_tools(&self, registry: &mut ToolRegistry) {
        for (_, info) in &self.loaded {
            for tool_def in &info.manifest.tools {
                let wrapper = PluginToolWrapper {
                    name: tool_def.name.clone(),
                    description: tool_def.description.clone(),
                    parameters: tool_def.parameters.clone(),
                    process: info.process.clone(),
                };
                registry.register(Box::new(wrapper));
                info!("Registered plugin tool: {} (from {})", tool_def.name, info.manifest.plugin.name);
            }
        }
    }

    /// Unload all plugins (kill processes).
    pub fn unload_all(&mut self) {
        self.loaded.clear();
        info!("All plugins unloaded");
    }

    /// Get list of loaded plugin names.
    pub fn loaded_names(&self) -> Vec<String> {
        self.loaded.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_manifest() {
        let toml_content = r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
description = "A test plugin"
executable = "./plugin.sh"

[[tools]]
name = "echo"
description = "Echo back input"
parameters = { type = "object", properties = { message = { type = "string" } } }
"#;
        let manifest: PluginManifest = toml::from_str(toml_content).unwrap();
        assert_eq!(manifest.plugin.name, "test-plugin");
        assert_eq!(manifest.plugin.version, "0.1.0");
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.tools[0].name, "echo");
    }

    #[test]
    fn test_discover_empty_dir() {
        let dir = std::env::temp_dir().join("arli-test-plugins-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let manager = PluginManager::new(dir.clone());
        let plugins = manager.discover().unwrap();
        assert!(plugins.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_plugin() {
        let dir = std::env::temp_dir().join("arli-test-plugins-discover");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let plugin_dir = dir.join("test-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        let manifest_content = r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
executable = "./plugin.sh"
"#;
        std::fs::write(plugin_dir.join("plugin.toml"), manifest_content).unwrap();

        // Create dummy executable
        std::fs::write(plugin_dir.join("plugin.sh"), "#!/bin/bash\necho '{}'\n").unwrap();

        let manager = PluginManager::new(dir.clone());
        let plugins = manager.discover().unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].plugin.name, "test-plugin");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
