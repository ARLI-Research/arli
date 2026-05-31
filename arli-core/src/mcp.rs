//! MCP (Model Context Protocol) server implementation.
//!
//! Implements a JSON-RPC 2.0 server over stdin/stdout that exposes
//! ARLI's tools as MCP tools for use by MCP clients (Claude Desktop, etc.).
//!
//! Supported methods:
//!   - initialize        → client handshake
//!   - notifications/initialized → client signals ready
//!   - tools/list        → list available tools with JSON schemas
//!   - tools/call        → execute a tool with arguments
//!
//! Reference: https://spec.modelcontextprotocol.io/

use crate::tools::ToolRegistry;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use tracing::{info, warn, error};

// ── JSON-RPC 2.0 message types ──

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

// ── MCP protocol types ──

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "arli-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
struct InitializeResult {
    protocolVersion: String,
    capabilities: ServerCapabilities,
    serverInfo: ServerInfo,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServerCapabilities {
    tools: ToolsCapability,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolsCapability {}

#[derive(Debug, Serialize, Deserialize)]
struct ServerInfo {
    name: String,
    version: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolsListResult {
    tools: Vec<McpTool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct McpTool {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Debug, Serialize)]
struct ToolCallResult {
    content: Vec<ToolContent>,
    #[serde(rename = "isError", skip_serializing_if = "std::ops::Not::not")]
    is_error: bool,
}

#[derive(Debug, Serialize)]
struct ToolContent {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

/// The MCP server — holds tools and handles JSON-RPC messages.
pub struct McpServer {
    tools: Arc<ToolRegistry>,
    initialized: bool,
}

impl McpServer {
    pub fn new(tools: ToolRegistry) -> Self {
        Self {
            tools: Arc::new(tools),
            initialized: false,
        }
    }

    /// Process a single JSON-RPC request and return a response (or None for notifications).
    fn handle_request(&mut self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let method = req.method.as_str();

        match method {
            "initialize" => {
                let result = InitializeResult {
                    protocolVersion: PROTOCOL_VERSION.to_string(),
                    capabilities: ServerCapabilities {
                        tools: ToolsCapability {},
                    },
                    serverInfo: ServerInfo {
                        name: SERVER_NAME.to_string(),
                        version: SERVER_VERSION.to_string(),
                    },
                };
                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: Some(serde_json::to_value(result).unwrap_or_default()),
                    error: None,
                })
            }

            "notifications/initialized" => {
                // Client signals it's ready — no response needed
                self.initialized = true;
                None
            }

            "tools/list" => {
                let tools: Vec<McpTool> = self
                    .tools
                    .schemas()
                    .into_iter()
                    .map(|s| McpTool {
                        name: s.function.name,
                        description: s.function.description,
                        input_schema: s.function.parameters,
                    })
                    .collect();

                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: Some(serde_json::to_value(ToolsListResult { tools }).unwrap_or_default()),
                    error: None,
                })
            }

            "tools/call" => {
                // tools/call requires async execution (tool.execute())
                // We handle this in the run loop, not here.
                // Return a placeholder — run_sync() intercepts this method.
                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32603,
                        message: "Internal: tools/call must be handled in run loop".to_string(),
                        data: None,
                    }),
                })
            }

            "ping" => {
                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: Some(Value::Object(serde_json::Map::new())),
                    error: None,
                })
            }

            _ => {
                warn!("Unknown MCP method: {}", method);
                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Method not found: {}", method),
                        data: None,
                    }),
                })
            }
        }
    }

    /// Run the MCP server — reads JSON-RPC from stdin, writes responses to stdout.
    ///
    /// This is a synchronous function suitable for the CLI entry point.
    /// Tool execution is done via blocking on the async runtime.
    pub fn run_sync(&mut self) -> anyhow::Result<()> {
        info!("MCP server starting (stdio transport)...");

        let stdin = std::io::stdin();
        let reader = BufReader::new(stdin.lock());

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    error!("Stdin read error: {}", e);
                    break;
                }
            };

            if line.trim().is_empty() {
                continue;
            }

            let req: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to parse JSON-RPC request: {} — line: {}", e, line);
                    let err_resp = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32700,
                            message: format!("Parse error: {}", e),
                            data: None,
                        }),
                    };
                    writeln!(std::io::stdout(), "{}", serde_json::to_string(&err_resp)?)?;
                    std::io::stdout().flush()?;
                    continue;
                }
            };

            // Check for tools/call — needs async execution
            if req.method == "tools/call" {
                let params: ToolCallParams = match req.params.as_ref().and_then(|p| {
                    serde_json::from_value(p.clone()).ok()
                }) {
                    Some(p) => p,
                    None => {
                        let err_resp = JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id: req.id.clone(),
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: "Invalid params".to_string(),
                                data: None,
                            }),
                        };
                        writeln!(std::io::stdout(), "{}", serde_json::to_string(&err_resp)?)?;
                        std::io::stdout().flush()?;
                        continue;
                    }
                };

                let args_str = serde_json::to_string(&params.arguments).unwrap_or_default();
                let tools = self.tools.clone();
                let req_id = req.id.clone();

                // Execute tool via blocking on tokio runtime
                let rt = tokio::runtime::Runtime::new()?;
                let output = rt.block_on(tools.execute(&params.name, &args_str));

                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req_id,
                    result: Some(serde_json::to_value(ToolCallResult {
                        content: vec![ToolContent {
                            content_type: "text".to_string(),
                            text: if output.success {
                                output.content
                            } else {
                                output.error.unwrap_or_else(|| "Unknown error".to_string())
                            },
                        }],
                        is_error: !output.success,
                    })
                    .unwrap_or_default()),
                    error: None,
                };

                writeln!(std::io::stdout(), "{}", serde_json::to_string(&resp)?)?;
                std::io::stdout().flush()?;
                continue;
            }

            // Handle all other methods synchronously
            if let Some(resp) = self.handle_request(req) {
                writeln!(std::io::stdout(), "{}", serde_json::to_string(&resp)?)?;
                std::io::stdout().flush()?;
            }
        }

        info!("MCP server stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;
    use async_trait::async_trait;

    struct TestTool;
    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str { "test.echo" }
        fn description(&self) -> &str { "Echo test tool" }
        fn parameters_schema(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string"}
                }
            })
        }
        async fn execute(&self, args: &str) -> crate::tools::ToolOutput {
            crate::tools::ToolOutput {
                success: true,
                content: format!("echo: {}", args),
                error: None,
            }
        }
    }

    #[test]
    fn test_initialize() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool));
        let mut server = McpServer::new(registry);

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(1.into())),
            method: "initialize".to_string(),
            params: None,
        };

        let resp = server.handle_request(req).unwrap();
        let result: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.protocolVersion, "2024-11-05");
        assert_eq!(result.serverInfo.name, "arli-mcp");
    }

    #[test]
    fn test_tools_list() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool));
        let mut server = McpServer::new(registry);

        // Initialize first
        let init_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(1.into())),
            method: "initialize".to_string(),
            params: None,
        };
        server.handle_request(init_req);

        // List tools
        let list_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(2.into())),
            method: "tools/list".to_string(),
            params: None,
        };

        let resp = server.handle_request(list_req).unwrap();
        let result: ToolsListResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "test.echo");
    }

    #[test]
    fn test_ping() {
        let registry = ToolRegistry::new();
        let mut server = McpServer::new(registry);

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(1.into())),
            method: "ping".to_string(),
            params: None,
        };

        let resp = server.handle_request(req).unwrap();
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_unknown_method() {
        let registry = ToolRegistry::new();
        let mut server = McpServer::new(registry);

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(1.into())),
            method: "nonexistent".to_string(),
            params: None,
        };

        let resp = server.handle_request(req).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn test_notification_no_response() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool));
        let mut server = McpServer::new(registry);

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "notifications/initialized".to_string(),
            params: None,
        };

        let resp = server.handle_request(req);
        assert!(resp.is_none());
        assert!(server.initialized);
    }
}
