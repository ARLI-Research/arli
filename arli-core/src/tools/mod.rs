pub mod browser;
pub mod builtin;
pub mod delegate;
pub mod execute_code;
pub mod http_get;
pub mod image_gen;
pub mod memory;
pub mod patch;
pub mod process;
pub mod search;
pub mod search_files;
pub mod toolsets;
pub mod video_gen;
pub mod vision;
pub mod voice;
pub mod web_search;

pub use delegate::DelegateTaskTool;
pub use image_gen::ImageGenTool;
pub use memory::MemoryTool;

use crate::providers::ToolSchema;
use async_trait::async_trait;
use std::collections::HashMap;

/// Result of executing a tool
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub success: bool,
    pub content: String,
    pub error: Option<String>,
}

/// The Tool trait — every tool implements this.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn execute(&self, arguments: &str) -> ToolOutput;

    fn to_schema(&self) -> ToolSchema {
        ToolSchema {
            schema_type: "function".to_string(),
            function: crate::providers::FunctionSchema {
                name: self.name().to_string(),
                description: self.description().to_string(),
                parameters: self.parameters_schema(),
            },
        }
    }
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        tracing::info!("Registered tool: {}", name);
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|t| t.to_schema()).collect()
    }

    pub async fn execute(&self, name: &str, arguments: &str) -> ToolOutput {
        match self.get(name) {
            Some(tool) => tool.execute(arguments).await,
            None => ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Tool not found: {}", name)),
            },
        }
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl ToolOutput {
    pub fn error(msg: &str) -> Self {
        Self {
            success: false,
            content: String::new(),
            error: Some(msg.to_string()),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
