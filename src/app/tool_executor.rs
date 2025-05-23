use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Span, debug, error, instrument};

use crate::api::ToolCall;
use crate::api::tools::Tool as ApiTool;
use crate::tools::{ToolError, traits::Tool as ToolTrait};

/// Manages the execution of tools called by the AI model
#[derive(Clone)]
pub struct ToolExecutor {
    pub(crate) registry: Arc<HashMap<String, Arc<dyn ToolTrait>>>,
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

        match self.registry.get(tool_name) {
            Some(tool) => {
                debug!(target: "app.tool_executor.execute_tool_with_cancellation", "Executing tool {} ({}) via registry with cancellation", tool_name, tool_id);
                // Pass the token to the trait method
                tool.execute(tool_call.parameters.clone(), Some(token))
                    .await
            }
            None => {
                error!(
                    target: "app.tool_executor.execute_tool_with_cancellation",
                    "{}",
                    format!("Unknown tool called: {} ({})", tool_name, tool_id)
                );
                Err(ToolError::UnknownTool(tool_name.clone()))
            }
        }
    }
}
