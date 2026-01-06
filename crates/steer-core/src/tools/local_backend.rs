use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use steer_tools::ToolCall;

use crate::config::LlmConfigProvider;
use crate::tools::{BackendMetadata, ExecutionContext, ToolBackend};
use steer_tools::tools::{read_only_workspace_tools, workspace_tools};
use steer_tools::{
    ExecutionContext as SteerExecutionContext, ToolError, ToolSchema, result::ToolResult,
    traits::ExecutableTool,
};

pub struct LocalBackend {
    registry: HashMap<String, Box<dyn ExecutableTool>>,
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalBackend {
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    pub fn from_tools(tools: Vec<Box<dyn ExecutableTool>>) -> Self {
        let mut registry = HashMap::new();
        tools.into_iter().for_each(|tool| {
            registry.insert(tool.name().to_string(), tool);
        });
        Self { registry }
    }

    pub fn with_tools(
        tool_names: Vec<String>,
        _llm_config_provider: Arc<LlmConfigProvider>,
        _workspace: Arc<dyn crate::workspace::Workspace>,
    ) -> Self {
        let all_tools = workspace_tools();

        let filtered_tools: Vec<Box<dyn ExecutableTool>> = all_tools
            .into_iter()
            .filter(|tool| tool_names.contains(&tool.name().to_string()))
            .collect();

        Self::from_tools(filtered_tools)
    }

    pub fn without_tools(
        excluded_tools: Vec<String>,
        _llm_config_provider: Arc<LlmConfigProvider>,
        _workspace: Arc<dyn crate::workspace::Workspace>,
    ) -> Self {
        let all_tools = workspace_tools();

        let filtered_tools: Vec<Box<dyn ExecutableTool>> = all_tools
            .into_iter()
            .filter(|tool| !excluded_tools.contains(&tool.name().to_string()))
            .collect();

        Self::from_tools(filtered_tools)
    }

    pub fn full(
        _llm_config_provider: Arc<LlmConfigProvider>,
        _workspace: Arc<dyn crate::workspace::Workspace>,
    ) -> Self {
        Self::from_tools(workspace_tools())
    }

    pub fn read_only(_llm_config_provider: Arc<LlmConfigProvider>) -> Self {
        Self::from_tools(read_only_workspace_tools())
    }

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
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .registry
            .get(&tool_call.name)
            .ok_or_else(|| ToolError::UnknownTool(tool_call.name.clone()))?;

        let steer_context = SteerExecutionContext::new(tool_call.id.clone())
            .with_cancellation_token(context.cancellation_token.clone());

        tool.run(tool_call.parameters.clone(), &steer_context).await
    }

    async fn supported_tools(&self) -> Vec<String> {
        self.registry.keys().cloned().collect()
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_backend_creation() {
        let backend = LocalBackend::new();
        assert_eq!(backend.registry.len(), 0);
    }

    #[tokio::test]
    async fn test_local_backend_metadata() {
        let backend = LocalBackend::new();
        let metadata = backend.metadata();
        assert_eq!(metadata.name, "Local");
        assert_eq!(metadata.backend_type, "Local");
        assert_eq!(metadata.location, Some("localhost".to_string()));
    }
}
