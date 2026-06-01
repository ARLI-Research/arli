use std::collections::{HashMap, HashSet};

/// Toolset — a named group of tools that can be enabled/disabled together.
///
/// Similar to Hermes' toolsets.py. Each toolset maps to a set of tool names.
/// The agent only exposes tools from enabled toolsets to the LLM.
#[derive(Debug, Clone)]
pub struct Toolset {
    pub name: String,
    pub description: String,
    pub tools: HashSet<String>,
}

/// Toolset registry manages which toolsets are active.
#[derive(Debug, Clone)]
pub struct ToolsetRegistry {
    toolsets: HashMap<String, Toolset>,
    enabled: HashSet<String>,
}

impl ToolsetRegistry {
    pub fn new() -> Self {
        Self {
            toolsets: HashMap::new(),
            enabled: HashSet::new(),
        }
    }

    /// Register a toolset definition.
    pub fn register(&mut self, toolset: Toolset) {
        let name = toolset.name.clone();
        self.toolsets.insert(name.clone(), toolset);
        self.enabled.insert(name);
    }

    /// Enable a toolset by name.
    pub fn enable(&mut self, name: &str) {
        self.enabled.insert(name.to_string());
    }

    /// Disable a toolset by name.
    pub fn disable(&mut self, name: &str) {
        self.enabled.remove(name);
    }

    /// Get all tool names from enabled toolsets.
    pub fn enabled_tool_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        for ts_name in &self.enabled {
            if let Some(ts) = self.toolsets.get(ts_name) {
                names.extend(ts.tools.iter().cloned());
            }
        }
        names
    }

    /// Check if a tool is currently enabled.
    pub fn is_tool_enabled(&self, tool_name: &str) -> bool {
        self.enabled_tool_names().contains(tool_name)
    }

    /// List all toolsets with status
    pub fn list(&self) -> Vec<(String, bool, String)> {
        self.toolsets
            .iter()
            .map(|(name, ts)| {
                (
                    name.clone(),
                    self.enabled.contains(name),
                    ts.description.clone(),
                )
            })
            .collect()
    }
}

impl Default for ToolsetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Pre-defined toolsets that ship with ARLI.
pub fn default_toolsets() -> ToolsetRegistry {
    let mut registry = ToolsetRegistry::new();

    registry.register(Toolset {
        name: "core".to_string(),
        description: "Essential tools: file I/O, shell, search".into(),
        tools: ["read_file", "write_file", "shell", "search_files"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    });

    registry.register(Toolset {
        name: "web".to_string(),
        description: "Web access: HTTP requests, browser".into(),
        tools: ["http_get"].iter().map(|s| s.to_string()).collect(),
    });

    registry.register(Toolset {
        name: "session".to_string(),
        description: "Session tools: search past conversations".into(),
        tools: ["session_search"].iter().map(|s| s.to_string()).collect(),
    });

    registry
}
