use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Span, instrument};

use crate::api::ToolCall;
use crate::api::tools::Tool as ApiTool;
use crate::tools::{ToolError, traits::Tool as ToolTrait};
use crate::utils::logging;

/// Manages the execution of tools called by Claude
#[derive(Clone)]
pub struct ToolExecutor {
    pub(crate) registry: Arc<HashMap<String, Arc<dyn ToolTrait>>>,
}

impl ToolExecutor {
    /// Create a new tool executor with the default set of tools
    pub fn new() -> Self {
        super::tool_registry::ToolExecutorBuilder::standard().build()
    }

    /// Create a tool executor with read-only tools
    pub fn read_only() -> Self {
        super::tool_registry::ToolExecutorBuilder::read_only().build()
    }

    /// Create a tool executor with a custom registry builder
    pub fn with_builder(builder: super::tool_registry::ToolExecutorBuilder) -> Self {
        builder.build()
    }

    /// Get a list of all available tools (metadata).
    pub fn available_tools(&self) -> Vec<&dyn ToolTrait> {
        self.registry.values().map(|t| t.as_ref()).collect()
    }

    /// Convert registry tools to API tool descriptions
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

        logging::debug(
            "app.tool_executor.to_api_tools",
            &format!("Converting registry tools to API tools: {:?}", api_tools),
        );
        return api_tools;
    }

    /// Execute a tool call with cancellation support using the registry and trait.
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
                crate::utils::logging::debug(
                    "app.tool_executor.execute_tool_with_cancellation",
                    &format!(
                        "Executing tool {} ({}) via registry with cancellation",
                        tool_name, tool_id
                    ),
                );
                // Pass the token to the trait method
                tool.execute(tool_call.parameters.clone(), Some(token))
                    .await
            }
            None => {
                crate::utils::logging::error(
                    "app.tool_executor.execute_tool_with_cancellation",
                    &format!("Unknown tool called: {} ({})", tool_name, tool_id),
                );
                Err(ToolError::UnknownTool(tool_name.clone()))
            }
        }
    }
}
