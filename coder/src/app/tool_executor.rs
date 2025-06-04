use anyhow::Result;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Span, debug, error, instrument};

use crate::api::ToolCall;
use crate::app::validation::{ValidationContext, ValidatorRegistry};
use crate::tools::{BackendRegistry, ExecutionContext, ToolBackend};
use tools::ToolSchema as ApiTool;
use tools::ToolError;

/// Manages the execution of tools called by the AI model
#[derive(Clone)]
pub struct ToolExecutor {
    pub(crate) backend_registry: Arc<BackendRegistry>,
    pub(crate) validators: Arc<ValidatorRegistry>,
}

impl ToolExecutor {
    pub fn new(backend_registry: Arc<BackendRegistry>) -> Self {
        Self {
            backend_registry,
            validators: Arc::new(ValidatorRegistry::new()),
        }
    }

    pub fn with_validators(
        backend_registry: Arc<BackendRegistry>,
        validators: Arc<ValidatorRegistry>,
    ) -> Self {
        Self {
            backend_registry,
            validators,
        }
    }

    pub fn requires_approval(&self, tool_name: &str) -> Result<bool> {
        // Check if any backend supports this tool
        if self
            .backend_registry
            .get_backend_for_tool(tool_name)
            .is_some()
        {
            // Only bash requires approval for now
            Ok(tool_name == "bash")
        } else {
            Err(anyhow::anyhow!("Unknown tool: {}", tool_name))
        }
    }

    pub fn to_api_tools(&self) -> Vec<ApiTool> {
        // Get tools dynamically from the backend registry
        self.backend_registry.to_api_tools()
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
                session_id: "default".to_string(), // TODO: Get real session ID
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
