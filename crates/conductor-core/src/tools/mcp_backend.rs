//! MCP backend implementation using the official rmcp crate
//!
//! This module provides the ToolBackend implementation for MCP servers.

use async_trait::async_trait;
use rmcp::transport::ConfigureCommandExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

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
    transport::TokioChildProcess,
};

/// Tool backend for executing tools via MCP servers
pub struct McpBackend {
    server_name: String,
    command: String,
    args: Vec<String>,
    tool_filter: ToolFilter,
    client: Arc<RwLock<Option<RunningService<RoleClient, ()>>>>,
    tools: Arc<RwLock<HashMap<String, Tool>>>,
}

impl McpBackend {
    /// Create a new MCP backend
    pub async fn new(
        server_name: String,
        command: String,
        args: Vec<String>,
        tool_filter: ToolFilter,
    ) -> Result<Self, ToolError> {
        info!(
            "Creating MCP backend '{}' with command: {} {:?}",
            server_name, command, args
        );

        let mut child_process = TokioChildProcess::new(Command::new(&command).configure(|cmd| {
            cmd.args(&args);
        }))
        .map_err(|e| {
            error!("Failed to create MCP process: {}", e);
            ToolError::mcp_connection_failed(
                &server_name,
                format!("Failed to create MCP process: {}", e),
            )
        })?;

        if let Some(stderr) = child_process.take_stderr() {
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

        let client = ().serve(child_process).await.map_err(|e| {
            error!("Failed to serve MCP: {}", e);
            ToolError::mcp_connection_failed(&server_name, format!("Failed to serve MCP: {}", e))
        })?;

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
                        format!("Failed to list tools: {}", e),
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
            command,
            args,
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
                ToolError::execution(&tool_call.name, format!("Tool execution failed: {}", e))
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
        BackendMetadata::new(self.server_name.clone(), "MCP".to_string())
            .with_info("command".to_string(), self.command.clone())
            .with_info("args".to_string(), self.args.join(" "))
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

    #[test]
    fn test_tool_name_extraction() {
        let prefix = "mcp__test__";
        let full_name = "mcp__test__some_tool";
        let actual_name = if full_name.starts_with(prefix) {
            &full_name[prefix.len()..]
        } else {
            full_name
        };

        assert_eq!(actual_name, "some_tool");
    }
}
