//! MCP backend implementation using the official rmcp crate
//!
//! This module provides the ToolBackend implementation for MCP servers.

use async_trait::async_trait;
use rmcp::transport::ConfigureCommandExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

use crate::api::ToolCall;
use crate::session::state::ToolFilter;
use crate::tools::{BackendMetadata, ExecutionContext, ToolBackend};
use conductor_tools::{
    InputSchema, ToolError, ToolSchema,
    result::{ExternalResult, ToolResult},
};

use rmcp::{
    model::{CallToolRequestParam, Tool},
    service::{RoleClient, RunningService, ServiceExt},
    transport::{SseClientTransport, StreamableHttpClientTransport, TokioChildProcess},
};

/// MCP transport configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    /// Standard I/O transport (child process)
    Stdio { command: String, args: Vec<String> },
    /// TCP transport
    Tcp { host: String, port: u16 },
    /// Unix domain socket transport
    #[cfg(unix)]
    Unix { path: String },
    /// Server-Sent Events transport
    Sse {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<HashMap<String, String>>,
    },
    /// HTTP streamable transport
    Http {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<HashMap<String, String>>,
    },
}

/// Tool backend for executing tools via MCP servers
pub struct McpBackend {
    server_name: String,
    transport: McpTransport,
    tool_filter: ToolFilter,
    client: Arc<RwLock<Option<RunningService<RoleClient, ()>>>>,
    tools: Arc<RwLock<HashMap<String, Tool>>>,
}

impl McpBackend {
    /// Create a new MCP backend
    pub async fn new(
        server_name: String,
        transport: McpTransport,
        tool_filter: ToolFilter,
    ) -> Result<Self, ToolError> {
        info!(
            "Creating MCP backend '{}' with transport: {:?}",
            server_name, transport
        );

        let client = match &transport {
            McpTransport::Stdio { command, args } => {
                let (transport, stderr) =
                    TokioChildProcess::builder(Command::new(command).configure(|cmd| {
                        cmd.args(args);
                    }))
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| {
                        error!("Failed to create MCP process: {}", e);
                        ToolError::mcp_connection_failed(
                            &server_name,
                            format!("Failed to create MCP process: {e}"),
                        )
                    })?;

                if let Some(stderr) = stderr {
                    let server_name_for_logging = server_name.clone();
                    tokio::spawn(async move {
                        use tokio::io::{AsyncBufReadExt, BufReader};
                        let mut reader = BufReader::new(stderr);
                        let mut line = String::new();

                        while let Ok(len) = reader.read_line(&mut line).await {
                            if len == 0 {
                                break;
                            }
                            debug!(
                                target: "mcp_server",
                                "[{}] {}",
                                server_name_for_logging,
                                line.trim()
                            );
                            line.clear();
                        }
                    });
                }

                ().serve(transport).await.map_err(|e| {
                    error!("Failed to serve MCP: {}", e);
                    ToolError::mcp_connection_failed(
                        &server_name,
                        format!("Failed to serve MCP: {e}"),
                    )
                })?
            }
            McpTransport::Tcp { host, port } => {
                let stream = TcpStream::connect((host.as_str(), *port))
                    .await
                    .map_err(|e| {
                        error!("Failed to connect to TCP MCP server: {}", e);
                        ToolError::mcp_connection_failed(
                            &server_name,
                            format!("Failed to connect to {host}:{port} - {e}"),
                        )
                    })?;

                ().serve(stream).await.map_err(|e| {
                    error!("Failed to serve MCP over TCP: {}", e);
                    ToolError::mcp_connection_failed(
                        &server_name,
                        format!("Failed to serve MCP over TCP: {e}"),
                    )
                })?
            }
            #[cfg(unix)]
            McpTransport::Unix { path } => {
                let stream = UnixStream::connect(path).await.map_err(|e| {
                    error!("Failed to connect to Unix socket MCP server: {}", e);
                    ToolError::mcp_connection_failed(
                        &server_name,
                        format!("Failed to connect to Unix socket {path} - {e}"),
                    )
                })?;

                ().serve(stream).await.map_err(|e| {
                    error!("Failed to serve MCP over Unix socket: {}", e);
                    ToolError::mcp_connection_failed(
                        &server_name,
                        format!("Failed to serve MCP over Unix socket: {e}"),
                    )
                })?
            }
            McpTransport::Sse { url, headers } => {
                // Use the dedicated SSE client transport for SSE connections
                if headers.is_some() && !headers.as_ref().unwrap().is_empty() {
                    info!(
                        "SSE transport with custom headers requested; headers may not be applied"
                    );
                }

                let transport = SseClientTransport::start(url.clone()).await.map_err(|e| {
                    error!("Failed to start SSE transport: {}", e);
                    ToolError::mcp_connection_failed(
                        &server_name,
                        format!("Failed to start SSE transport: {e}"),
                    )
                })?;

                ().serve(transport).await.map_err(|e| {
                    error!("Failed to serve MCP over SSE: {}", e);
                    ToolError::mcp_connection_failed(
                        &server_name,
                        format!("Failed to serve MCP over SSE: {e}"),
                    )
                })?
            }
            McpTransport::Http { url, headers } => {
                // Use the simpler from_uri method
                let transport = StreamableHttpClientTransport::from_uri(url.clone());

                if headers.is_some() && !headers.as_ref().unwrap().is_empty() {
                    info!(
                        "HTTP transport with custom headers requested; headers may not be applied"
                    );
                }

                ().serve(transport).await.map_err(|e| {
                    error!("Failed to serve MCP over HTTP: {}", e);
                    ToolError::mcp_connection_failed(
                        &server_name,
                        format!("Failed to serve MCP over HTTP: {e}"),
                    )
                })?
            }
        };

        let server_info = client.peer_info();
        info!("Connected to server: {server_info:#?}");

        debug!("Attempting to list tools from MCP server '{}'", server_name);

        let list_tools_timeout = std::time::Duration::from_secs(10);
        let tool_list =
            tokio::time::timeout(list_tools_timeout, client.list_tools(Default::default()))
                .await
                .map_err(|_| {
                    ToolError::mcp_connection_failed(
                        &server_name,
                        "Timeout listing tools".to_string(),
                    )
                })?
                .map_err(|e| {
                    ToolError::mcp_connection_failed(
                        &server_name,
                        format!("Failed to list tools: {e}"),
                    )
                })?;

        // Process the tools
        let mut tools = HashMap::new();
        for tool in tool_list.tools {
            tools.insert(tool.name.to_string(), tool);
        }

        info!(
            "Discovered {} tools from MCP server '{}': {}",
            tools.len(),
            server_name,
            tools
                .keys()
                .map(|k| k.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        let backend = Self {
            server_name,
            transport,
            tool_filter,
            client: Arc::new(RwLock::new(Some(client))),
            tools: Arc::new(RwLock::new(tools)),
        };

        Ok(backend)
    }

    /// Apply tool filter to determine if a tool should be included
    fn should_include_tool(&self, tool_name: &str) -> bool {
        match &self.tool_filter {
            ToolFilter::All => true,
            ToolFilter::Include(included) => included.contains(&tool_name.to_string()),
            ToolFilter::Exclude(excluded) => !excluded.contains(&tool_name.to_string()),
        }
    }

    fn mcp_tool_to_schema(&self, tool: &Tool) -> ToolSchema {
        let description = match &tool.description {
            Some(desc) if !desc.is_empty() => desc.to_string(),
            _ => format!(
                "Tool '{}' from MCP server '{}'",
                tool.name, self.server_name
            ),
        };

        // Convert Arc<Map> to InputSchema
        let properties = (*tool.input_schema).clone();
        let required = properties
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let input_schema = InputSchema {
            properties: properties
                .get("properties")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default(),
            required,
            schema_type: "object".to_string(),
        };

        ToolSchema {
            name: format!("mcp__{}__{}", self.server_name, tool.name),
            description,
            input_schema,
        }
    }
}

#[async_trait]
impl ToolBackend for McpBackend {
    async fn execute(
        &self,
        tool_call: &ToolCall,
        _context: &ExecutionContext,
    ) -> Result<ToolResult, ToolError> {
        // Get the service
        let service_guard = self.client.read().await;
        let service = service_guard
            .as_ref()
            .ok_or_else(|| ToolError::execution("mcp", "MCP service not initialized"))?;

        // Extract the actual tool name (remove mcp_servername_ prefix)
        let prefix = format!("mcp__{}__", self.server_name);
        let actual_tool_name = if tool_call.name.starts_with(&prefix) {
            &tool_call.name[prefix.len()..]
        } else {
            &tool_call.name
        };

        debug!(
            "Executing tool '{}' via MCP server '{}'",
            actual_tool_name, self.server_name
        );

        // Convert parameters to a Map if it's an object
        let arguments = if let Some(obj) = tool_call.parameters.as_object() {
            Some(obj.clone())
        } else if tool_call.parameters.is_null() {
            None
        } else {
            return Err(ToolError::invalid_params(
                &tool_call.name,
                "Parameters must be an object",
            ));
        };

        // Execute the tool
        let result = service
            .call_tool(CallToolRequestParam {
                name: actual_tool_name.to_string().into(),
                arguments,
            })
            .await
            .map_err(|e| {
                ToolError::execution(&tool_call.name, format!("Tool execution failed: {e}"))
            })?;

        // Convert result to string
        let output = result
            .content
            .into_iter()
            .map(|content| {
                // Access the raw content
                match &content.raw {
                    rmcp::model::RawContent::Text(text_content) => text_content.text.to_string(),
                    rmcp::model::RawContent::Image { .. } => "[Image content]".to_string(),
                    rmcp::model::RawContent::Resource { .. } => "[Resource content]".to_string(),
                    rmcp::model::RawContent::Audio { .. } => "[Audio content]".to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Return as external tool result
        Ok(ToolResult::External(ExternalResult {
            tool_name: tool_call.name.clone(),
            payload: output,
        }))
    }

    async fn supported_tools(&self) -> Vec<String> {
        let tools = self.tools.read().await;
        tools
            .keys()
            .filter(|tool_name| self.should_include_tool(tool_name))
            .map(|tool_name| format!("mcp__{}__{}", self.server_name, tool_name))
            .collect()
    }

    async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|tool| self.should_include_tool(&tool.name))
            .map(|tool| self.mcp_tool_to_schema(tool))
            .collect()
    }

    fn metadata(&self) -> BackendMetadata {
        let mut metadata = BackendMetadata::new(self.server_name.clone(), "MCP".to_string());

        match &self.transport {
            McpTransport::Stdio { command, args } => {
                metadata = metadata
                    .with_info("transport".to_string(), "stdio".to_string())
                    .with_info("command".to_string(), command.clone())
                    .with_info("args".to_string(), args.join(" "));
            }
            McpTransport::Tcp { host, port } => {
                metadata = metadata
                    .with_info("transport".to_string(), "tcp".to_string())
                    .with_info("host".to_string(), host.clone())
                    .with_info("port".to_string(), port.to_string());
            }
            #[cfg(unix)]
            McpTransport::Unix { path } => {
                metadata = metadata
                    .with_info("transport".to_string(), "unix".to_string())
                    .with_info("path".to_string(), path.clone());
            }
            McpTransport::Sse { url, .. } => {
                metadata = metadata
                    .with_info("transport".to_string(), "sse".to_string())
                    .with_info("url".to_string(), url.clone());
            }
            McpTransport::Http { url, .. } => {
                metadata = metadata
                    .with_info("transport".to_string(), "http".to_string())
                    .with_info("url".to_string(), url.clone());
            }
        }

        metadata
    }

    async fn health_check(&self) -> bool {
        // Check if service is connected
        let service_guard = self.client.read().await;
        service_guard.is_some()
    }

    async fn requires_approval(&self, _tool_name: &str) -> Result<bool, ToolError> {
        // MCP tools generally require approval unless we have specific information
        // In the future, we could query the MCP server for tool metadata
        Ok(true)
    }
}

impl Drop for McpBackend {
    fn drop(&mut self) {
        // Schedule cleanup in a detached task
        let service = self.client.clone();

        tokio::spawn(async move {
            if let Some(service) = service.write().await.take() {
                let _ = service.cancel().await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_name_extraction() {
        let prefix = "mcp__test__";
        let full_name = "mcp__test__some_tool";
        let actual_name = if let Some(stripped) = full_name.strip_prefix(prefix) {
            stripped
        } else {
            full_name
        };

        assert_eq!(actual_name, "some_tool");
    }

    #[test]
    fn test_mcp_transport_serialization() {
        // Test stdio transport
        let stdio = McpTransport::Stdio {
            command: "python".to_string(),
            args: vec!["-m".to_string(), "test_server".to_string()],
        };
        let json = serde_json::to_string(&stdio).unwrap();
        assert!(json.contains("\"type\":\"stdio\""));
        assert!(json.contains("\"command\":\"python\""));

        // Test TCP transport
        let tcp = McpTransport::Tcp {
            host: "localhost".to_string(),
            port: 3000,
        };
        let json = serde_json::to_string(&tcp).unwrap();
        assert!(json.contains("\"type\":\"tcp\""));
        assert!(json.contains("\"host\":\"localhost\""));
        assert!(json.contains("\"port\":3000"));

        // Test Unix transport
        #[cfg(unix)]
        {
            let unix = McpTransport::Unix {
                path: "/tmp/test.sock".to_string(),
            };
            let json = serde_json::to_string(&unix).unwrap();
            assert!(json.contains("\"type\":\"unix\""));
            assert!(json.contains("\"path\":\"/tmp/test.sock\""));
        }
    }

    #[test]
    fn test_mcp_transport_deserialization() {
        // Test stdio transport
        let json = r#"{"type":"stdio","command":"node","args":["server.js"]}"#;
        let transport: McpTransport = serde_json::from_str(json).unwrap();
        match transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "node");
                assert_eq!(args, vec!["server.js"]);
            }
            _ => unreachable!("Stdio transport"),
        }

        // Test TCP transport
        let json = r#"{"type":"tcp","host":"127.0.0.1","port":8080}"#;
        let transport: McpTransport = serde_json::from_str(json).unwrap();
        match transport {
            McpTransport::Tcp { host, port } => {
                assert_eq!(host, "127.0.0.1");
                assert_eq!(port, 8080);
            }
            _ => unreachable!("TCP transport"),
        }
    }
}
