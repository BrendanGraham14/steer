use crate::app::domain::types::{SessionId, ToolCallId};
use crate::config::LlmConfigProvider;
use crate::tools::error::Result;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{Span, debug, error, instrument};

use crate::app::validation::{ValidationContext, ValidatorRegistry};
use crate::tools::registry::ToolRegistry;
use crate::tools::resolver::BackendResolver;
use crate::tools::services::ToolServices;
use crate::tools::static_tool::{StaticToolContext, StaticToolError};
use crate::tools::{BackendRegistry, ExecutionContext};
use crate::workspace::Workspace;
use steer_tools::{ToolCall, ToolSchema, result::ToolResult};

#[derive(Clone)]
pub struct ToolExecutor {
    pub(crate) workspace: Arc<dyn Workspace>,
    pub(crate) backend_registry: Arc<BackendRegistry>,
    pub(crate) validators: Arc<ValidatorRegistry>,
    pub(crate) llm_config_provider: Option<LlmConfigProvider>,
    pub(crate) tool_registry: Option<Arc<ToolRegistry>>,
    pub(crate) tool_services: Option<Arc<ToolServices>>,
}

impl ToolExecutor {
    pub fn with_workspace(workspace: Arc<dyn Workspace>) -> Self {
        Self {
            workspace,
            backend_registry: Arc::new(BackendRegistry::new()),
            validators: Arc::new(ValidatorRegistry::new()),
            llm_config_provider: None,
            tool_registry: None,
            tool_services: None,
        }
    }

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
            tool_registry: None,
            tool_services: None,
        }
    }

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
            tool_registry: None,
            tool_services: None,
        }
    }

    pub fn with_static_tools(
        mut self,
        registry: Arc<ToolRegistry>,
        services: Arc<ToolServices>,
    ) -> Self {
        self.tool_registry = Some(registry);
        self.tool_services = Some(services);
        self
    }

    pub async fn requires_approval(&self, tool_name: &str) -> Result<bool> {
        if let Some(registry) = &self.tool_registry
            && registry.is_static_tool(tool_name)
        {
            return Ok(registry.requires_approval(tool_name));
        }

        let workspace_tools = self.workspace.available_tools().await;
        if workspace_tools.iter().any(|t| t.name == tool_name) {
            return Ok(self.workspace.requires_approval(tool_name).await?);
        }

        match self.backend_registry.get_backend_for_tool(tool_name) {
            Some(backend) => Ok(backend.requires_approval(tool_name).await?),
            None => Err(steer_tools::ToolError::UnknownTool(tool_name.to_string()).into()),
        }
    }

    pub async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        self.get_tool_schemas_with_capabilities(super::Capabilities::all())
            .await
    }

    pub async fn get_tool_schemas_with_resolver(
        &self,
        session_resolver: Option<&dyn BackendResolver>,
    ) -> Vec<ToolSchema> {
        self.get_tool_schemas_with_capabilities_and_resolver(
            super::Capabilities::all(),
            session_resolver,
        )
        .await
    }

    pub async fn get_tool_schemas_with_capabilities(
        &self,
        capabilities: super::Capabilities,
    ) -> Vec<ToolSchema> {
        self.get_tool_schemas_with_capabilities_and_resolver(capabilities, None)
            .await
    }

    pub async fn get_tool_schemas_with_capabilities_and_resolver(
        &self,
        capabilities: super::Capabilities,
        session_resolver: Option<&dyn BackendResolver>,
    ) -> Vec<ToolSchema> {
        let mut schemas = Vec::new();
        let mut static_tool_names = std::collections::HashSet::new();

        if let Some(registry) = &self.tool_registry {
            let static_schemas = registry.available_schemas(capabilities).await;
            for schema in &static_schemas {
                static_tool_names.insert(schema.name.clone());
            }
            schemas.extend(static_schemas);
        }

        for schema in self.workspace.available_tools().await {
            if !static_tool_names.contains(&schema.name) {
                schemas.push(schema);
            }
        }

        if let Some(resolver) = session_resolver {
            for schema in resolver.get_tool_schemas().await {
                if !static_tool_names.contains(&schema.name) {
                    schemas.push(schema);
                }
            }
        }

        for schema in self.backend_registry.get_tool_schemas().await {
            if !static_tool_names.contains(&schema.name) {
                schemas.push(schema);
            }
        }

        schemas
    }

    pub fn is_static_tool(&self, tool_name: &str) -> bool {
        self.tool_registry
            .as_ref()
            .is_some_and(|r| r.is_static_tool(tool_name))
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

    #[instrument(skip(self, tool_call, session_id, token), fields(tool.name = %tool_call.name, tool.id = %tool_call.id))]
    pub async fn execute_tool_with_session(
        &self,
        tool_call: &ToolCall,
        session_id: SessionId,
        token: CancellationToken,
    ) -> std::result::Result<ToolResult, steer_tools::ToolError> {
        self.execute_tool_with_session_resolver(tool_call, session_id, token, None)
            .await
    }

    #[instrument(skip(self, tool_call, session_id, token, session_resolver), fields(tool.name = %tool_call.name, tool.id = %tool_call.id))]
    pub async fn execute_tool_with_session_resolver(
        &self,
        tool_call: &ToolCall,
        session_id: SessionId,
        token: CancellationToken,
        session_resolver: Option<&dyn BackendResolver>,
    ) -> std::result::Result<ToolResult, steer_tools::ToolError> {
        let tool_name = &tool_call.name;

        if let Some((registry, services)) =
            self.tool_registry.as_ref().zip(self.tool_services.as_ref())
            && let Some(tool) = registry.static_tool(tool_name)
        {
            debug!(target: "tool_executor", "Executing static tool: {}", tool_name);
            return self
                .execute_static_tool(tool, tool_call, session_id, services, token)
                .await;
        }

        self.execute_tool_with_resolver(tool_call, token, session_resolver)
            .await
    }

    #[instrument(skip(self, tool_call, token), fields(tool.name = %tool_call.name, tool.id = %tool_call.id))]
    pub async fn execute_tool_with_cancellation(
        &self,
        tool_call: &ToolCall,
        token: CancellationToken,
    ) -> std::result::Result<ToolResult, steer_tools::ToolError> {
        self.execute_tool_with_resolver(tool_call, token, None)
            .await
    }

    #[instrument(skip(self, tool_call, token, session_resolver), fields(tool.name = %tool_call.name, tool.id = %tool_call.id))]
    pub async fn execute_tool_with_resolver(
        &self,
        tool_call: &ToolCall,
        token: CancellationToken,
        session_resolver: Option<&dyn BackendResolver>,
    ) -> std::result::Result<ToolResult, steer_tools::ToolError> {
        let tool_name = &tool_call.name;
        let tool_id = &tool_call.id;

        Span::current().record("tool.name", tool_name);
        Span::current().record("tool.id", tool_id);

        if let Some(validator) = self.validators.get_validator(tool_name) {
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
        }

        let mut builder = ExecutionContext::builder(
            "default".to_string(),
            "default".to_string(),
            tool_call.id.clone(),
            token,
        );

        if let Some(provider) = &self.llm_config_provider {
            builder = builder.llm_config_provider(provider.clone());
        }

        let context = builder.build();

        let workspace_tools = self.workspace.available_tools().await;
        if workspace_tools.iter().any(|t| &t.name == tool_name) {
            debug!(target: "tool_executor", "Executing workspace tool: {} ({})", tool_name, tool_id);
            return self
                .execute_workspace_tool(&self.workspace, tool_call, &context)
                .await;
        }

        if let Some(resolver) = session_resolver
            && let Some(backend) = resolver.resolve(tool_name).await
        {
            debug!(target: "tool_executor", "Executing session MCP tool: {} ({})", tool_name, tool_id);
            return backend.execute(tool_call, &context).await;
        }

        let backend = self
            .backend_registry
            .get_backend_for_tool(tool_name)
            .cloned()
            .ok_or_else(|| {
                error!(target: "tool_executor", "No backend for tool: {} ({})", tool_name, tool_id);
                steer_tools::ToolError::UnknownTool(tool_name.clone())
            })?;

        debug!(target: "tool_executor", "Executing external tool: {} ({})", tool_name, tool_id);
        backend.execute(tool_call, &context).await
    }

    async fn execute_static_tool(
        &self,
        tool: &dyn super::static_tool::StaticToolErased,
        tool_call: &ToolCall,
        session_id: SessionId,
        services: &Arc<ToolServices>,
        token: CancellationToken,
    ) -> std::result::Result<ToolResult, steer_tools::ToolError> {
        let ctx = StaticToolContext {
            tool_call_id: ToolCallId(tool_call.id.clone()),
            session_id,
            cancellation_token: token,
            services: services.clone(),
        };

        let output = tool
            .execute_erased(tool_call.parameters.clone(), &ctx)
            .await
            .map_err(|e| match e {
                StaticToolError::InvalidParams(msg) => {
                    steer_tools::ToolError::InvalidParams(tool_call.name.clone(), msg)
                }
                StaticToolError::Execution(msg) => steer_tools::ToolError::Execution {
                    tool_name: tool_call.name.clone(),
                    message: msg,
                },
                StaticToolError::MissingCapability(cap) => {
                    steer_tools::ToolError::InternalError(format!("Missing capability: {cap}"))
                }
                StaticToolError::Cancelled => {
                    steer_tools::ToolError::Cancelled(tool_call.name.clone())
                }
                StaticToolError::Io(msg) => steer_tools::ToolError::Io {
                    tool_name: tool_call.name.clone(),
                    message: msg,
                },
            })?;

        Ok(output)
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
                // Map WorkspaceError variants to structured ToolError
                use steer_workspace::WorkspaceError;
                match e {
                    WorkspaceError::ToolExecution(msg) => steer_tools::ToolError::Execution {
                        tool_name: tool_call.name.clone(),
                        message: msg,
                    },
                    WorkspaceError::Io(msg) => steer_tools::ToolError::Io {
                        tool_name: tool_call.name.clone(),
                        message: msg,
                    },
                    _ => steer_tools::ToolError::InternalError(e.to_string()),
                }
            })
    }
}
