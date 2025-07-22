use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::api::ToolCall;
use crate::config::LlmConfigProvider;
use crate::tools::{BackendMetadata, ExecutionContext, ToolBackend};
use crate::tools::{DispatchAgentTool, FetchTool};
use steer_tools::tools::{read_only_workspace_tools, workspace_tools};
use steer_tools::{
    ExecutionContext as SteerExecutionContext, Tool, ToolError, ToolSchema, result::ToolResult,
    traits::ExecutableTool,
};

// Tool wrappers for server-side tools
struct FetchToolWrapper(FetchTool);
struct DispatchAgentToolWrapper(DispatchAgentTool);

#[async_trait]
impl Tool for FetchToolWrapper {
    type Output = ToolResult;

    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn description(&self) -> String {
        self.0.description()
    }

    fn input_schema(&self) -> &'static steer_tools::InputSchema {
        self.0.input_schema()
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SteerExecutionContext,
    ) -> Result<Self::Output, ToolError> {
        let result = self.0.execute(params, ctx).await?;
        Ok(ToolResult::Fetch(result))
    }

    fn requires_approval(&self) -> bool {
        self.0.requires_approval()
    }
}

#[async_trait]
impl Tool for DispatchAgentToolWrapper {
    type Output = ToolResult;

    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn description(&self) -> String {
        self.0.description()
    }

    fn input_schema(&self) -> &'static steer_tools::InputSchema {
        self.0.input_schema()
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SteerExecutionContext,
    ) -> Result<Self::Output, ToolError> {
        let result = self.0.execute(params, ctx).await?;
        Ok(ToolResult::Agent(result))
    }

    fn requires_approval(&self) -> bool {
        self.0.requires_approval()
    }
}

/// Local backend that executes tools in the current process
///
/// This backend uses the steer-tools implementations directly.
pub struct LocalBackend {
    /// The tool registry containing all available tools
    registry: HashMap<String, Box<dyn ExecutableTool>>,
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalBackend {
    /// Create a new empty LocalBackend
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    /// Create a backend from a collection of tool instances
    pub fn from_tools(tools: Vec<Box<dyn ExecutableTool>>) -> Self {
        let mut registry = HashMap::new();
        tools.into_iter().for_each(|tool| {
            registry.insert(tool.name().to_string(), tool);
        });
        Self { registry }
    }

    /// Create a backend with only specific tools enabled by name
    ///
    /// This method takes a list of tool names and creates a backend
    /// containing only those tools from the full set of available tools.
    pub fn with_tools(
        tool_names: Vec<String>,
        llm_config_provider: Arc<LlmConfigProvider>,
        workspace: Arc<dyn crate::workspace::Workspace>,
    ) -> Self {
        let mut all_tools = workspace_tools();
        all_tools.push(Box::new(FetchToolWrapper(FetchTool {
            llm_config_provider: llm_config_provider.clone(),
        })));
        all_tools.push(Box::new(DispatchAgentToolWrapper(DispatchAgentTool {
            llm_config_provider: llm_config_provider.clone(),
            workspace,
        })));

        let filtered_tools: Vec<Box<dyn ExecutableTool>> = all_tools
            .into_iter()
            .filter(|tool| tool_names.contains(&tool.name().to_string()))
            .collect();

        Self::from_tools(filtered_tools)
    }

    /// Create a backend excluding specific tools by name
    ///
    /// This method takes a list of tool names to exclude and creates a backend
    /// containing all other tools from the full set of available tools.
    pub fn without_tools(
        excluded_tools: Vec<String>,
        llm_config_provider: Arc<LlmConfigProvider>,
        workspace: Arc<dyn crate::workspace::Workspace>,
    ) -> Self {
        let mut all_tools = workspace_tools();
        all_tools.push(Box::new(FetchToolWrapper(FetchTool {
            llm_config_provider: llm_config_provider.clone(),
        })));
        all_tools.push(Box::new(DispatchAgentToolWrapper(DispatchAgentTool {
            llm_config_provider: llm_config_provider.clone(),
            workspace,
        })));

        let filtered_tools: Vec<Box<dyn ExecutableTool>> = all_tools
            .into_iter()
            .filter(|tool| !excluded_tools.contains(&tool.name().to_string()))
            .collect();

        Self::from_tools(filtered_tools)
    }

    /// Create a new LocalBackend with all tools (workspace + server tools)
    pub fn full(
        llm_config_provider: Arc<LlmConfigProvider>,
        workspace: Arc<dyn crate::workspace::Workspace>,
    ) -> Self {
        let mut tools = workspace_tools();
        // Add server-side tools
        tools.push(Box::new(FetchToolWrapper(FetchTool {
            llm_config_provider: llm_config_provider.clone(),
        })));
        tools.push(Box::new(DispatchAgentToolWrapper(DispatchAgentTool {
            llm_config_provider: llm_config_provider.clone(),
            workspace,
        })));
        Self::from_tools(tools)
    }

    /// Create a LocalBackend with only server-side tools
    pub fn server_only(
        llm_config_provider: Arc<LlmConfigProvider>,
        workspace: Arc<dyn crate::workspace::Workspace>,
    ) -> Self {
        Self::from_tools(vec![
            Box::new(FetchToolWrapper(FetchTool {
                llm_config_provider: llm_config_provider.clone(),
            })),
            Box::new(DispatchAgentToolWrapper(DispatchAgentTool {
                llm_config_provider: llm_config_provider.clone(),
                workspace,
            })),
        ])
    }

    /// Create a LocalBackend with read-only tools
    ///
    /// This creates a backend with only read-only tools, useful for
    /// sandboxed or restricted execution environments.
    pub fn read_only(llm_config_provider: Arc<LlmConfigProvider>) -> Self {
        let mut tools = read_only_workspace_tools();
        // Add server-side tools (they're read-only too)
        tools.push(Box::new(FetchToolWrapper(FetchTool {
            llm_config_provider: llm_config_provider.clone(),
        })));
        Self::from_tools(tools)
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
    ) -> Result<ToolResult, ToolError> {
        // Get the tool from the registry
        let tool = self
            .registry
            .get(&tool_call.name)
            .ok_or_else(|| ToolError::UnknownTool(tool_call.name.clone()))?;

        // Create execution context for steer-tools
        let steer_context = SteerExecutionContext::new(tool_call.id.clone())
            .with_cancellation_token(context.cancellation_token.clone());

        // Execute the tool and get the result
        tool.run(tool_call.parameters.clone(), &steer_context).await
    }

    async fn supported_tools(&self) -> Vec<String> {
        // Return the tools we currently have in the registry
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
