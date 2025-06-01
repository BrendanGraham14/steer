use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Span, debug, error, instrument};

use crate::api::ToolCall;
use crate::api::tools::Tool as ApiTool;
use crate::tools::{ToolError, traits::Tool as ToolTrait, BackendRegistry, ExecutionContext};

/// Manages the execution of tools called by the AI model
#[derive(Clone)]
pub struct ToolExecutor {
    pub(crate) registry: Arc<HashMap<String, Arc<dyn ToolTrait>>>,
    pub(crate) backend_registry: Option<Arc<BackendRegistry>>,
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExecutor {
    pub fn new() -> Self {
        super::tool_registry::ToolExecutorBuilder::standard().build()
    }

    pub fn read_only() -> Self {
        super::tool_registry::ToolExecutorBuilder::read_only().build()
    }

    pub fn with_builder(builder: super::tool_registry::ToolExecutorBuilder) -> Self {
        builder.build()
    }

    pub fn available_tools(&self) -> Vec<&dyn ToolTrait> {
        self.registry.values().map(|t| t.as_ref()).collect()
    }

    pub fn requires_approval(&self, tool_name: &str) -> Result<bool> {
        match self.registry.get(tool_name) {
            Some(tool) => Ok(tool.requires_approval()),
            None => Err(anyhow::anyhow!("Unknown tool: {}", tool_name)),
        }
    }

    pub fn to_api_tools(&self) -> Vec<ApiTool> {
        let api_tools = self
            .registry
            .values()
            .map(|tool| ApiTool {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                input_schema: tool.input_schema().clone(),
            })
            .collect();

        api_tools
    }

    /// Get the list of supported tools from the backend registry
    pub fn supported_tools(&self) -> Vec<String> {
        if let Some(backend_registry) = &self.backend_registry {
            backend_registry.supported_tools()
        } else {
            Vec::new()
        }
    }

    /// Set the backend registry for this tool executor
    /// 
    /// When a backend registry is set, the executor will first check if
    /// a backend can handle the tool before falling back to local execution.
    /// 
    /// # Arguments
    /// * `backend_registry` - The backend registry to use for tool routing
    pub fn with_backend_registry(mut self, backend_registry: Arc<BackendRegistry>) -> Self {
        self.backend_registry = Some(backend_registry);
        self
    }

    /// Get the backend registry if one is set
    pub fn backend_registry(&self) -> Option<&Arc<BackendRegistry>> {
        self.backend_registry.as_ref()
    }

    /// Check if backend routing is enabled
    pub fn has_backend_routing(&self) -> bool {
        self.backend_registry.is_some()
    }

    #[instrument(skip(self, tool_call, token), fields(tool.name = %tool_call.name, tool.id = %tool_call.id))]
    pub async fn execute_tool_with_cancellation(
        &self,
        tool_call: &ToolCall,
        token: CancellationToken,
    ) -> Result<String, ToolError> {
        let tool_name = &tool_call.name;
        let tool_id = &tool_call.id;

        Span::current().record("tool.name", tool_name);
        Span::current().record("tool.id", tool_id);

        // Check if we have a backend registry configured
        let backend_registry = self.backend_registry.as_ref()
            .ok_or_else(|| {
                error!(
                    target: "app.tool_executor.execute_tool_with_cancellation",
                    "No backend registry configured for tool executor"
                );
                ToolError::InternalError("No backend registry configured".to_string())
            })?;

        // Get the backend for this tool
        let backend = backend_registry.get_backend_for_tool(tool_name)
            .ok_or_else(|| {
                error!(
                    target: "app.tool_executor.execute_tool_with_cancellation",
                    "No backend configured for tool: {} ({})", 
                    tool_name, 
                    tool_id
                );
                ToolError::UnknownTool(tool_name.clone())
            })?;

        debug!(
            target: "app.tool_executor.execute_tool_with_cancellation",
            "Executing tool {} ({}) via backend with cancellation", 
            tool_name, 
            tool_id
        );
        
        // Create execution context for the backend
        let context = ExecutionContext::new(
            "default".to_string(), // TODO: Get real session ID
            "default".to_string(), // TODO: Get real operation ID  
            tool_call.id.clone(),
            token,
        );
        
        backend.execute(tool_call, &context).await
    }
}
