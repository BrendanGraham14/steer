use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::conversation::{Message, MessageData, UserContent};
use crate::app::domain::runtime::{
    AgentConfig, AgentInterpreter, AgentInterpreterConfig, AgentInterpreterError,
};
use crate::app::domain::session::EventStore;
use crate::model_registry::ModelRegistry;
use crate::session::state::{BackendConfig, SessionConfig, SessionToolConfig, WorkspaceConfig};
use crate::tools::{McpBackend, SessionMcpBackends, ToolExecutor};
use crate::workspace::Workspace;
use tracing::warn;

use super::services::{AgentSpawner, SubAgentConfig, SubAgentError, SubAgentResult};

pub struct DefaultAgentSpawner {
    event_store: Arc<dyn EventStore>,
    api_client: Arc<ApiClient>,
    workspace: Arc<dyn Workspace>,
    model_registry: Arc<ModelRegistry>,
}

impl DefaultAgentSpawner {
    pub fn new(
        event_store: Arc<dyn EventStore>,
        api_client: Arc<ApiClient>,
        workspace: Arc<dyn Workspace>,
        model_registry: Arc<ModelRegistry>,
    ) -> Self {
        Self {
            event_store,
            api_client,
            workspace,
            model_registry,
        }
    }
}

#[async_trait]
impl AgentSpawner for DefaultAgentSpawner {
    async fn spawn(
        &self,
        config: SubAgentConfig,
        cancel_token: CancellationToken,
    ) -> Result<SubAgentResult, SubAgentError> {
        let workspace = config
            .workspace
            .clone()
            .unwrap_or_else(|| self.workspace.clone());
        let workspace_path = workspace.working_directory().to_path_buf();

        let mut session_config = SessionConfig::read_only(config.model.clone());
        session_config.workspace = WorkspaceConfig::Local { path: workspace_path };
        session_config.workspace_ref = config.workspace_ref.clone();
        session_config.workspace_id = config.workspace_id;
        session_config.repo_ref = config.repo_ref.clone();
        session_config.workspace_name = config.workspace_name.clone();
        session_config.parent_session_id = Some(config.parent_session_id);
        let mut tool_config = SessionToolConfig::read_only();
        tool_config.backends = config.mcp_backends.clone();
        session_config.tool_config = tool_config;

        let session_backends = if config.allow_mcp_tools && !config.mcp_backends.is_empty() {
            let session_backends = Arc::new(SessionMcpBackends::new());
            for backend_config in &config.mcp_backends {
                let BackendConfig::Mcp {
                    server_name,
                    transport,
                    tool_filter,
                } = backend_config;

                match McpBackend::new(
                    server_name.clone(),
                    transport.clone(),
                    tool_filter.clone(),
                )
                .await
                {
                    Ok(backend) => {
                        session_backends
                            .register(server_name.clone(), Arc::new(backend))
                            .await;
                    }
                    Err(err) => {
                        warn!(
                            server_name = %server_name,
                            error = %err,
                            "Failed to start MCP server for sub-agent"
                        );
                    }
                }
            }
            Some(session_backends)
        } else {
            None
        };

        let interpreter_config = AgentInterpreterConfig {
            auto_approve_tools: true,
            parent_session_id: Some(config.parent_session_id),
            session_config: Some(session_config),
            session_backends: session_backends.clone(),
        };

        let tool_executor = Arc::new(ToolExecutor::with_workspace(workspace));

        let interpreter = match AgentInterpreter::new(
            self.event_store.clone(),
            self.api_client.clone(),
            tool_executor.clone(),
            interpreter_config,
        )
        .await
        {
            Ok(interpreter) => interpreter,
            Err(e) => {
                if let Some(backends) = session_backends.as_ref() {
                    backends.clear().await;
                }
                return Err(match e {
            AgentInterpreterError::EventStore(msg) => SubAgentError::EventStore(msg),
            AgentInterpreterError::Api(msg) => SubAgentError::Api(msg),
            AgentInterpreterError::Agent(msg) => SubAgentError::Agent(msg),
            AgentInterpreterError::Cancelled => SubAgentError::Cancelled,
                });
            }
        };

        let session_id = interpreter.session_id();

        let model_config = self
            .model_registry
            .get(&config.model)
            .ok_or_else(|| SubAgentError::Agent(format!("Model not found: {:?}", config.model)))?
            .clone();

        let resolver = session_backends
            .as_ref()
            .map(|backends| backends.as_ref() as &dyn crate::tools::BackendResolver);

        let available_tools: Vec<_> = tool_executor
            .get_tool_schemas_with_resolver(resolver)
            .await
            .into_iter()
            .filter(|schema| {
                config.allowed_tools.contains(&schema.name)
                    || (config.allow_mcp_tools && schema.name.starts_with("mcp__"))
            })
            .collect();

        let agent_config = AgentConfig {
            model: (model_config.provider.clone(), model_config.id.clone()),
            system_prompt: config.system_prompt,
            tools: available_tools,
        };

        let initial_messages = vec![Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: config.prompt,
                }],
            },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            parent_message_id: None,
        }];

        let final_message = interpreter
            .run(agent_config, initial_messages, None, cancel_token)
            .await
            .map_err(|e| match e {
                AgentInterpreterError::EventStore(msg) => SubAgentError::EventStore(msg),
                AgentInterpreterError::Api(msg) => SubAgentError::Api(msg),
                AgentInterpreterError::Agent(msg) => SubAgentError::Agent(msg),
                AgentInterpreterError::Cancelled => SubAgentError::Cancelled,
            });

        if let Some(backends) = session_backends.as_ref() {
            backends.clear().await;
        }

        let final_message = final_message?;

        Ok(SubAgentResult {
            session_id,
            final_message,
        })
    }
}
