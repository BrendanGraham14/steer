use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::api::ToolCall;
use crate::tools::{
    BackendMetadata, ExecutionContext, ToolBackend, ToolError, traits::Tool as ToolTrait,
};

/// Local backend that executes tools in the current process
///
/// This backend uses the existing tool registry to execute tools directly
/// in the local environment. It serves as the default/fallback backend
/// and is equivalent to the current tool execution behavior.
pub struct LocalBackend {
    /// The tool registry containing all available tools
    registry: Arc<HashMap<String, Arc<dyn ToolTrait>>>,
}

impl LocalBackend {
    /// Create a new LocalBackend with the given tool registry
    ///
    /// # Arguments
    /// * `registry` - The tool registry to use for tool execution
    pub fn new(registry: Arc<HashMap<String, Arc<dyn ToolTrait>>>) -> Self {
        Self { registry }
    }

    /// Create a LocalBackend with the standard tool set
    ///
    /// This is a convenience method that creates a backend with all
    /// the standard tools available in the application.
    pub fn standard() -> Self {
        let tool_executor = crate::app::tool_registry::ToolExecutorBuilder::standard().build();
        Self::new(tool_executor.registry)
    }

    /// Create a LocalBackend with read-only tools
    ///
    /// This creates a backend with only read-only tools, useful for
    /// sandboxed or restricted execution environments.
    pub fn read_only() -> Self {
        let tool_executor = crate::app::tool_registry::ToolExecutorBuilder::read_only().build();
        Self::new(tool_executor.registry)
    }

    /// Get the tool registry
    pub fn registry(&self) -> &Arc<HashMap<String, Arc<dyn ToolTrait>>> {
        &self.registry
    }

    /// Check if a tool is available in this backend
    pub fn has_tool(&self, tool_name: &str) -> bool {
        self.registry.contains_key(tool_name)
    }

    /// Get a specific tool by name
    pub fn get_tool(&self, tool_name: &str) -> Option<&Arc<dyn ToolTrait>> {
        self.registry.get(tool_name)
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

        // Execute the tool using the existing trait method
        // Pass the cancellation token from the context
        tool.execute(
            tool_call.parameters.clone(),
            Some(context.cancellation_token.clone()),
        )
        .await
    }

    fn supported_tools(&self) -> Vec<&'static str> {
        // Return all tools in the registry
        // Note: We need to return 'static str, so we'll list the known standard tools
        // These are the actual tool names from the tool! macro definitions
        vec![
            "bash",
            "grep",
            "dispatch_agent",
            "glob",
            "ls",
            "read_file",
            "edit_file",
            "multi_edit_file",
            "write_file",
            "web_fetch",
            "TodoRead",
            "TodoWrite",
        ]
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
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::standard()
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
        let backend = LocalBackend::standard();
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
        let backend = LocalBackend::standard();
        let metadata = backend.metadata();

        assert_eq!(metadata.name, "Local");
        assert_eq!(metadata.backend_type, "Local");
        assert_eq!(metadata.location, Some("localhost".to_string()));
        assert!(metadata.additional_info.contains_key("tool_count"));
        assert!(metadata.additional_info.contains_key("execution_env"));
    }

    #[tokio::test]
    async fn test_local_backend_supported_tools() {
        let backend = LocalBackend::standard();
        let supported = backend.supported_tools();

        assert!(!supported.is_empty());
        assert!(supported.contains(&"bash"));
        assert!(supported.contains(&"read_file"));
        assert!(supported.contains(&"edit_file"));
    }

    #[tokio::test]
    async fn test_local_backend_health_check() {
        let backend = LocalBackend::standard();
        assert!(backend.health_check().await);

        // Test with empty registry
        let empty_backend = LocalBackend::new(Arc::new(HashMap::new()));
        assert!(!empty_backend.health_check().await);
    }

    #[tokio::test]
    async fn test_local_backend_execution_unknown_tool() {
        let backend = LocalBackend::standard();

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
}
