use anyhow::Result;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Span, debug, error, instrument};

use crate::api::ToolCall;
use crate::app::validation::{ValidationContext, ValidatorRegistry};
use crate::tools::{BackendRegistry, ExecutionContext};
use tools::ToolError;
use tools::ToolSchema;

/// Manages the execution of tools called by the AI model
#[derive(Clone)]
pub struct ToolExecutor {
    pub(crate) backend_registry: Arc<BackendRegistry>,
    pub(crate) validators: Arc<ValidatorRegistry>,
}

impl ToolExecutor {
    pub async fn requires_approval(&self, tool_name: &str) -> Result<bool> {
        match self.backend_registry.get_backend_for_tool(tool_name) {
            Some(backend) => backend
                .requires_approval(tool_name)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to check approval requirement: {}", e)),
            None => Err(anyhow::anyhow!("Unknown tool: {}", tool_name)),
        }
    }

    pub async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        self.backend_registry.get_tool_schemas().await
    }

    /// Get the list of supported tools from the backend registry
    pub fn supported_tools(&self) -> Vec<String> {
        self.backend_registry.supported_tools()
    }

    /// Get the backend registry
    pub fn backend_registry(&self) -> &Arc<BackendRegistry> {
        &self.backend_registry
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

        // Pre-execution validation
        if let Some(validator) = self.validators.get_validator(tool_name) {
            let validation_context = ValidationContext {
                cancellation_token: token.clone(),
                user_id: None,
            };

            let validation_result = validator
                .validate(tool_call, &validation_context)
                .await
                .map_err(|e| ToolError::InternalError(format!("Validation failed: {}", e)))?;

            if !validation_result.allowed {
                return Err(ToolError::InternalError(
                    validation_result
                        .reason
                        .unwrap_or_else(|| "Tool execution was denied".to_string()),
                ));
            }
        }

        // Get the backend for this tool
        let backend = {
            self.backend_registry
                .get_backend_for_tool(tool_name)
                .cloned()
                .ok_or_else(|| {
                    error!(
                        target: "app.tool_executor.execute_tool_with_cancellation",
                        "No backend configured for tool: {} ({})",
                        tool_name,
                        tool_id
                    );
                    ToolError::UnknownTool(tool_name.clone())
                })?
        };

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
