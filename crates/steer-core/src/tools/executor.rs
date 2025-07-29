use crate::config::LlmConfigProvider;
use crate::tools::error::Result;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Span, debug, error, instrument};

use crate::app::validation::{ValidationContext, ValidatorRegistry};
use crate::tools::{BackendRegistry, ExecutionContext};
use crate::workspace::Workspace;
use steer_tools::{ToolCall, ToolSchema, result::ToolResult};

/// Manages the execution of tools called by the AI model
#[derive(Clone)]
pub struct ToolExecutor {
    /// Optional workspace for executing workspace tools
    pub(crate) workspace: Arc<dyn Workspace>,
    /// Registry for external tool backends (MCP servers, etc.)
    pub(crate) backend_registry: Arc<BackendRegistry>,
    /// Validators for tool execution
    pub(crate) validators: Arc<ValidatorRegistry>,
    /// Provider for LLM configuration
    pub(crate) llm_config_provider: Option<LlmConfigProvider>,
}

impl ToolExecutor {
    /// Create a ToolExecutor with a workspace for workspace tools
    pub fn with_workspace(workspace: Arc<dyn Workspace>) -> Self {
        Self {
            workspace,
            backend_registry: Arc::new(BackendRegistry::new()),
            validators: Arc::new(ValidatorRegistry::new()),
            llm_config_provider: None,
        }
    }

    /// Create a ToolExecutor with custom components
    pub fn with_components(
        workspace: Arc<dyn Workspace>,
        backend_registry: Arc<BackendRegistry>,
        validators: Arc<ValidatorRegistry>,
    ) -> Self {
        Self {
            workspace,
            backend_registry,
            validators,
            llm_config_provider: None,
        }
    }

    /// Create a ToolExecutor with all components including LLM config provider
    pub fn with_all_components(
        workspace: Arc<dyn Workspace>,
        backend_registry: Arc<BackendRegistry>,
        validators: Arc<ValidatorRegistry>,
        llm_config_provider: LlmConfigProvider,
    ) -> Self {
        Self {
            workspace,
            backend_registry,
            validators,
            llm_config_provider: Some(llm_config_provider),
        }
    }

    pub async fn requires_approval(&self, tool_name: &str) -> Result<bool> {
        // First check if it's a workspace tool
        let workspace_tools = self.workspace.available_tools().await;
        if workspace_tools.iter().any(|t| t.name == tool_name) {
            return Ok(self.workspace.requires_approval(tool_name).await?);
        }

        // Otherwise check external backends
        match self.backend_registry.get_backend_for_tool(tool_name) {
            Some(backend) => Ok(backend.requires_approval(tool_name).await?),
            None => Err(steer_tools::ToolError::UnknownTool(tool_name.to_string()).into()),
        }
    }

    pub async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        let mut schemas = Vec::new();

        // Add workspace tools if available
        schemas.extend(self.workspace.available_tools().await);

        // Add external backend tools
        schemas.extend(self.backend_registry.get_tool_schemas().await);

        schemas
    }

    /// Get the list of supported tools
    pub async fn supported_tools(&self) -> Vec<String> {
        let schemas = self.get_tool_schemas().await;
        schemas.into_iter().map(|s| s.name).collect()
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
    ) -> std::result::Result<ToolResult, steer_tools::ToolError> {
        let tool_name = &tool_call.name;
        let tool_id = &tool_call.id;

        Span::current().record("tool.name", tool_name);
        Span::current().record("tool.id", tool_id);

        // Pre-execution validation
        if let Some(validator) = self.validators.get_validator(tool_name) {
            // Only validate if we have an LLM config provider
            if let Some(ref llm_config_provider) = self.llm_config_provider {
                let validation_context = ValidationContext {
                    cancellation_token: token.clone(),
                    llm_config_provider: llm_config_provider.clone(),
                };

                let validation_result = validator
                    .validate(tool_call, &validation_context)
                    .await
                    .map_err(|e| {
                        steer_tools::ToolError::InternalError(format!("Validation failed: {e}"))
                    })?;

                if !validation_result.allowed {
                    return Err(steer_tools::ToolError::InternalError(
                        validation_result
                            .reason
                            .unwrap_or_else(|| "Tool execution was denied".to_string()),
                    ));
                }
            }
            // If no LLM config provider, skip validation (allow execution)
        }

        // Create execution context
        let mut builder = ExecutionContext::builder(
            "default".to_string(), // TODO: Get real session ID
            "default".to_string(), // TODO: Get real operation ID
            tool_call.id.clone(),
            token,
        );

        // Add LLM config provider if available
        if let Some(provider) = &self.llm_config_provider {
            builder = builder.llm_config_provider(provider.clone());
        }

        let context = builder.build();

        // First check if it's a workspace tool
        let workspace_tools = self.workspace.available_tools().await;
        if workspace_tools.iter().any(|t| &t.name == tool_name) {
            debug!(
                target: "app.tool_executor.execute_tool_with_cancellation",
                "Executing workspace tool {} ({}) with cancellation",
                tool_name,
                tool_id
            );

            return self
                .execute_workspace_tool(&self.workspace, tool_call, &context)
                .await;
        }

        // Otherwise check external backends
        let backend = self
            .backend_registry
            .get_backend_for_tool(tool_name)
            .cloned()
            .ok_or_else(|| {
                error!(
                    target: "app.tool_executor.execute_tool_with_cancellation",
                    "No backend configured for tool: {} ({})",
                    tool_name,
                    tool_id
                );
                steer_tools::ToolError::UnknownTool(tool_name.clone())
            })?;

        debug!(
            target: "app.tool_executor.execute_tool_with_cancellation",
            "Executing external tool {} ({}) via backend with cancellation",
            tool_name,
            tool_id
        );

        backend.execute(tool_call, &context).await
    }

    /// Execute a tool directly without validation - for user-initiated bash commands
    #[instrument(skip(self, tool_call, token), fields(tool.name = %tool_call.name, tool.id = %tool_call.id))]
    pub async fn execute_tool_direct(
        &self,
        tool_call: &ToolCall,
        token: CancellationToken,
    ) -> std::result::Result<ToolResult, steer_tools::ToolError> {
        let tool_name = &tool_call.name;
        let tool_id = &tool_call.id;

        Span::current().record("tool.name", tool_name);
        Span::current().record("tool.id", tool_id);

        // Create execution context
        let mut builder = ExecutionContext::builder(
            "direct".to_string(), // Mark as direct execution
            "direct".to_string(),
            tool_call.id.clone(),
            token,
        );

        // Add LLM config provider if available
        if let Some(provider) = &self.llm_config_provider {
            builder = builder.llm_config_provider(provider.clone());
        }

        let context = builder.build();

        // First check if it's a workspace tool (no validation for direct execution)
        let workspace_tools = self.workspace.available_tools().await;
        if workspace_tools.iter().any(|t| &t.name == tool_name) {
            debug!(
                target: "app.tool_executor.execute_tool_direct",
                "Executing workspace tool {} ({}) directly (no validation)",
                tool_name,
                tool_id
            );

            return self
                .execute_workspace_tool(&self.workspace, tool_call, &context)
                .await;
        }

        // Otherwise check external backends
        let backend = self
            .backend_registry
            .get_backend_for_tool(tool_name)
            .cloned()
            .ok_or_else(|| {
                error!(
                    target: "app.tool_executor.execute_tool_direct",
                    "No backend configured for tool: {} ({})",
                    tool_name,
                    tool_id
                );
                steer_tools::ToolError::UnknownTool(tool_name.clone())
            })?;

        debug!(
            target: "app.tool_executor.execute_tool_direct",
            "Executing external tool {} ({}) directly via backend (no validation)",
            tool_name,
            tool_id
        );

        backend.execute(tool_call, &context).await
    }

    /// Helper method to execute a workspace tool
    async fn execute_workspace_tool(
        &self,
        workspace: &Arc<dyn Workspace>,
        tool_call: &ToolCall,
        context: &ExecutionContext,
    ) -> std::result::Result<ToolResult, steer_tools::ToolError> {
        // Convert ExecutionContext to steer-tools ExecutionContext
        let tools_context = steer_tools::ExecutionContext::new(context.tool_call_id.clone())
            .with_cancellation_token(context.cancellation_token.clone());

        workspace
            .execute_tool(tool_call, tools_context)
            .await
            .map_err(|e| {
                steer_tools::ToolError::InternalError(format!("Workspace execution failed: {e}"))
            })
    }
}
