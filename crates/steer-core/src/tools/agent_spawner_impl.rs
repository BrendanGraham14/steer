use std::collections::{HashMap, HashSet};
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
use crate::session::state::{
    ApprovalRules, BackendConfig, SessionConfig, SessionToolConfig, ToolApprovalPolicy,
    ToolVisibility, UnapprovedBehavior, WorkspaceConfig,
};
use crate::tools::{BackendResolver, McpBackend, SessionMcpBackends, ToolExecutor, ToolSystemBuilder};
use crate::workspace::{RepoManager, Workspace, WorkspaceManager};
use tracing::warn;

use super::services::{AgentSpawner, SubAgentConfig, SubAgentError, SubAgentResult};

pub struct DefaultAgentSpawner {
    event_store: Arc<dyn EventStore>,
    api_client: Arc<ApiClient>,
    workspace: Arc<dyn Workspace>,
    model_registry: Arc<ModelRegistry>,
    workspace_manager: Option<Arc<dyn WorkspaceManager>>,
    repo_manager: Option<Arc<dyn RepoManager>>,
}

impl DefaultAgentSpawner {
    pub fn new(
        event_store: Arc<dyn EventStore>,
        api_client: Arc<ApiClient>,
        workspace: Arc<dyn Workspace>,
        model_registry: Arc<ModelRegistry>,
        workspace_manager: Option<Arc<dyn WorkspaceManager>>,
        repo_manager: Option<Arc<dyn RepoManager>>,
    ) -> Self {
        Self {
            event_store,
            api_client,
            workspace,
            model_registry,
            workspace_manager,
            repo_manager,
        }
    }

    fn build_tool_executor(&self, workspace: Arc<dyn Workspace>) -> Arc<ToolExecutor> {
        let mut tool_builder = ToolSystemBuilder::new(
            workspace,
            self.event_store.clone(),
            self.api_client.clone(),
            self.model_registry.clone(),
        );

        if let Some(manager) = &self.workspace_manager {
            tool_builder = tool_builder.with_workspace_manager(manager.clone());
        }
        if let Some(manager) = &self.repo_manager {
            tool_builder = tool_builder.with_repo_manager(manager.clone());
        }

        tool_builder.build()
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

        let mut approved_tools: HashSet<String> =
            config.allowed_tools.iter().cloned().collect();
        let mut visibility_tools = approved_tools.clone();

        if let Some(backends) = session_backends.as_ref() {
            for schema in backends.get_tool_schemas().await {
                visibility_tools.insert(schema.name.clone());
                approved_tools.insert(schema.name);
            }
        }

        let approval_policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Deny,
            preapproved: ApprovalRules {
                tools: approved_tools,
                per_tool: HashMap::new(),
            },
        };

        let tool_config = SessionToolConfig {
            backends: config.mcp_backends.clone(),
            visibility: ToolVisibility::Whitelist(visibility_tools),
            approval_policy,
            metadata: HashMap::new(),
        };

        let mut session_config = SessionConfig::read_only(config.model.clone());
        session_config.workspace = WorkspaceConfig::Local {
            path: workspace_path,
        };
        session_config.workspace_ref = config.workspace_ref.clone();
        session_config.workspace_id = config.workspace_id;
        session_config.repo_ref = config.repo_ref.clone();
        session_config.workspace_name = config.workspace_name.clone();
        session_config.parent_session_id = Some(config.parent_session_id);
        session_config.tool_config = tool_config;

        let interpreter_config = AgentInterpreterConfig {
            auto_approve_tools: true,
            parent_session_id: Some(config.parent_session_id),
            session_config: Some(session_config),
            session_backends: session_backends.clone(),
        };

        let tool_executor = self.build_tool_executor(workspace);

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
            model: crate::config::model::ModelId::new(
                model_config.provider.clone(),
                model_config.id.clone(),
            ),
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

#[cfg(test)]
mod tests {
    use super::DefaultAgentSpawner;
    use crate::api::Client as ApiClient;
    use crate::app::domain::session::event_store::InMemoryEventStore;
    use crate::auth::ProviderRegistry;
    use crate::config::model::builtin;
    use crate::model_registry::ModelRegistry;
    use crate::session::state::ToolVisibility;
    use crate::test_utils::test_llm_config_provider;
    use crate::tools::services::AgentSpawner;
    use crate::tools::services::SubAgentConfig;
    use crate::workspace::WorkspaceConfig;
    use std::collections::HashSet;
    use std::sync::Arc;
    use steer_tools::tools::{
        BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME,
        VIEW_TOOL_NAME,
    };
    use tempfile::TempDir;

    #[tokio::test]
    async fn sub_agent_tool_executor_includes_static_tools() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let workspace = crate::workspace::create_workspace(&WorkspaceConfig::Local {
            path: temp_dir.path().to_path_buf(),
        })
        .await
        .expect("create workspace");
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).expect("model registry"));
        let provider_registry = Arc::new(ProviderRegistry::load(&[]).expect("provider registry"));
        let api_client = Arc::new(ApiClient::new_with_deps(
            test_llm_config_provider(),
            provider_registry,
            model_registry.clone(),
        ));

        let spawner = DefaultAgentSpawner::new(
            event_store,
            api_client,
            workspace.clone(),
            model_registry,
            None,
            None,
        );

        let tool_executor = spawner.build_tool_executor(workspace);
        for tool_name in [
            GLOB_TOOL_NAME,
            GREP_TOOL_NAME,
            LS_TOOL_NAME,
            VIEW_TOOL_NAME,
            EDIT_TOOL_NAME,
            MULTI_EDIT_TOOL_NAME,
            REPLACE_TOOL_NAME,
            BASH_TOOL_NAME,
        ] {
            assert!(
                tool_executor.is_static_tool(tool_name),
                "expected sub-agent to have static tool: {tool_name}"
            );
        }
    }
}
