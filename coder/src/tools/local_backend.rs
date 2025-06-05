use async_trait::async_trait;
use std::collections::HashMap;

use crate::api::ToolCall;
use crate::tools::{BackendMetadata, ExecutionContext, ToolBackend};
use crate::tools::{DispatchAgentTool, FetchTool};
use tools::tools::{read_only_workspace_tools, workspace_tools};
use tools::{ExecutionContext as CoderExecutionContext, Tool, ToolError, ToolSchema};

/// Local backend that executes tools in the current process
///
/// This backend uses the coder-tools implementations directly.
pub struct LocalBackend {
    /// The tool registry containing all available tools
    registry: HashMap<String, Box<dyn Tool>>,
}

impl LocalBackend {
    pub fn with_tools(tools: Vec<Box<dyn Tool>>) -> Self {
        let mut registry = HashMap::new();
        tools.into_iter().for_each(|tool| {
            registry.insert(tool.name().to_string(), tool);
        });
        Self { registry }
    }

    /// Create a new LocalBackend with all tools (workspace + server tools)
    pub fn full() -> Self {
        let mut tools = workspace_tools();
        // Add server-side tools
        tools.push(Box::new(FetchTool));
        tools.push(Box::new(DispatchAgentTool));
        Self::with_tools(tools)
    }

    /// Create a LocalBackend with only workspace tools
    pub fn workspace_only() -> Self {
        Self::with_tools(workspace_tools())
    }

    /// Create a LocalBackend with only server-side tools
    pub fn server_only() -> Self {
        Self::with_tools(vec![Box::new(FetchTool), Box::new(DispatchAgentTool)])
    }

    /// Create a LocalBackend with read-only tools
    ///
    /// This creates a backend with only read-only tools, useful for
    /// sandboxed or restricted execution environments.
    pub fn read_only() -> Self {
        let mut tools = read_only_workspace_tools();
        // Add server-side tools (they're read-only too)
        tools.push(Box::new(FetchTool));
        Self::with_tools(tools)
    }

    /// Check if a tool is available in this backend
    pub fn has_tool(&self, tool_name: &str) -> bool {
        self.registry.contains_key(tool_name)
    }
}

#[async_trait]
impl ToolBackend for LocalBackend {
    async fn execute(
        &self,
        tool_call: &ToolCall,
        context: &ExecutionContext,
    ) -> Result<String, ToolError> {
        // Get the tool from the registry
        let tool = self
            .registry
            .get(&tool_call.name)
            .ok_or_else(|| ToolError::UnknownTool(tool_call.name.clone()))?;

        // Create execution context for coder-tools
        let coder_context = CoderExecutionContext::new(tool_call.id.clone())
            .with_cancellation_token(context.cancellation_token.clone())
            .with_working_directory(std::env::current_dir().unwrap_or_else(|_| "/".into()));

        // Execute the tool directly
        tool.execute(tool_call.parameters.clone(), &coder_context)
            .await
    }

    fn supported_tools(&self) -> Vec<&'static str> {
        // Return the tools we currently have in the registry
        // We use a static list to match the trait requirement but ensure it reflects the registry
        let mut tool_names = Vec::new();

        for tool in self.registry.values() {
            tool_names.push(tool.name());
        }

        tool_names
    }

    async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        self.registry
            .iter()
            .map(|(name, tool)| ToolSchema {
                name: name.clone(),
                description: tool.description().to_string(),
                input_schema: tool.input_schema().clone(),
            })
            .collect()
    }

    fn metadata(&self) -> BackendMetadata {
        BackendMetadata::new("Local".to_string(), "Local".to_string())
            .with_location("localhost".to_string())
            .with_info("tool_count".to_string(), self.registry.len().to_string())
            .with_info("execution_env".to_string(), "current_process".to_string())
    }

    async fn health_check(&self) -> bool {
        // Local backend is always healthy if we can access the registry
        !self.registry.is_empty()
    }

    async fn requires_approval(&self, tool_name: &str) -> Result<bool, ToolError> {
        // Get the tool from the registry and check its requires_approval method
        self.registry
            .get(tool_name)
            .map(|tool| tool.requires_approval())
            .ok_or_else(|| ToolError::UnknownTool(tool_name.to_string()))
    }
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ToolCall;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn test_local_backend_creation() {
        let backend = LocalBackend::full();
        assert!(!backend.registry.is_empty());
        assert!(backend.has_tool("bash"));
        assert!(backend.has_tool("read_file"));
        assert!(!backend.has_tool("nonexistent_tool"));
    }

    #[tokio::test]
    async fn test_local_backend_read_only() {
        let backend = LocalBackend::read_only();
        assert!(!backend.registry.is_empty());
        assert!(backend.has_tool("read_file"));
        assert!(!backend.has_tool("bash")); // bash is not in read-only set
    }

    #[tokio::test]
    async fn test_local_backend_metadata() {
        let backend = LocalBackend::full();
        let metadata = backend.metadata();

        assert_eq!(metadata.name, "Local");
        assert_eq!(metadata.backend_type, "Local");
        assert_eq!(metadata.location, Some("localhost".to_string()));
        assert!(metadata.additional_info.contains_key("tool_count"));
        assert!(metadata.additional_info.contains_key("execution_env"));
    }

    #[tokio::test]
    async fn test_local_backend_supported_tools() {
        let backend = LocalBackend::full();
        let supported = backend.supported_tools();

        assert!(!supported.is_empty());
        assert!(supported.contains(&"bash"));
        assert!(supported.contains(&"read_file"));
        assert!(supported.contains(&"edit_file"));
    }

    #[tokio::test]
    async fn test_local_backend_health_check() {
        let backend = LocalBackend::full();
        assert!(backend.health_check().await);
    }

    #[tokio::test]
    async fn test_local_backend_execution_unknown_tool() {
        let backend = LocalBackend::full();

        let tool_call = ToolCall {
            name: "unknown_tool".to_string(),
            parameters: json!({}),
            id: "test_id".to_string(),
        };

        let context = ExecutionContext::new(
            "session".to_string(),
            "operation".to_string(),
            "tool_call".to_string(),
            CancellationToken::new(),
        );

        let result = backend.execute(&tool_call, &context).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            ToolError::UnknownTool(name) => assert_eq!(name, "unknown_tool"),
            _ => panic!("Expected UnknownTool error"),
        }
    }

    #[tokio::test]
    async fn test_local_backend_requires_approval() {
        let backend = LocalBackend::full();

        // Test a tool that typically requires approval (like bash)
        let result = backend.requires_approval("bash").await;
        assert!(result.is_ok());
        assert!(result.unwrap()); // bash should require approval

        // Test a tool that typically doesn't require approval (like read_file)
        let result = backend.requires_approval("read_file").await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // read_file should NOT require approval

        // Test an unknown tool
        let result = backend.requires_approval("unknown_tool").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::UnknownTool(name) => assert_eq!(name, "unknown_tool"),
            _ => panic!("Expected UnknownTool error"),
        }
    }
}
