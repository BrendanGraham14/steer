use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::domain::runtime::RuntimeService;
use crate::app::domain::session::EventStore;
use crate::error::Error;
use crate::model_registry::ModelRegistry;
use crate::runners::OneShotRunner;
use crate::session::state::{
    ApprovalRulesOverrides, SessionConfig, SessionPolicyOverrides, SessionToolConfig,
    ToolApprovalPolicy, ToolApprovalPolicyOverrides, ToolVisibility, UnapprovedBehavior,
    WorkspaceConfig,
};
use crate::tools::{ToolExecutor, ToolSystemBuilder};
use crate::workspace::{RepoManager, Workspace, WorkspaceManager};

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

        let visibility_tools: HashSet<String> = config.allowed_tools.iter().cloned().collect();
        let mcp_backends = if config.allow_mcp_tools {
            config.mcp_backends.clone()
        } else {
            Vec::new()
        };

        let tool_config = SessionToolConfig {
            backends: mcp_backends,
            visibility: ToolVisibility::All,
            approval_policy: ToolApprovalPolicy::default(),
            metadata: HashMap::new(),
        };

        let policy_overrides = SessionPolicyOverrides {
            default_model: Some(config.model.clone()),
            tool_visibility: Some(ToolVisibility::Whitelist(visibility_tools.clone())),
            approval_policy: ToolApprovalPolicyOverrides {
                default_behavior: Some(UnapprovedBehavior::Prompt),
                preapproved: ApprovalRulesOverrides {
                    tools: visibility_tools,
                    per_tool: HashMap::new(),
                },
            },
        };

        let session_config = SessionConfig {
            workspace: WorkspaceConfig::Local {
                path: workspace_path,
            },
            workspace_ref: config.workspace_ref.clone(),
            workspace_id: config.workspace_id,
            repo_ref: config.repo_ref.clone(),
            parent_session_id: Some(config.parent_session_id),
            workspace_name: config.workspace_name.clone(),
            tool_config,
            system_prompt: config
                .system_context
                .as_ref()
                .map(|context| context.prompt.clone()),
            primary_agent_id: None,
            policy_overrides,
            metadata: HashMap::new(),
            default_model: config.model.clone(),
        };

        let tool_executor = self.build_tool_executor(workspace);
        let runtime = RuntimeService::spawn(
            self.event_store.clone(),
            self.api_client.clone(),
            tool_executor,
        );

        let run_result = OneShotRunner::run_new_session_with_cancel(
            &runtime.handle,
            session_config,
            config.prompt,
            config.model.clone(),
            cancel_token,
        )
        .await;

        runtime.shutdown().await;

        let run_result = run_result.map_err(|err| match err {
            Error::Cancelled => SubAgentError::Cancelled,
            Error::Api(error) => SubAgentError::Api(error.to_string()),
            other => SubAgentError::Agent(other.to_string()),
        })?;

        Ok(SubAgentResult {
            session_id: run_result.session_id,
            final_message: run_result.final_message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DefaultAgentSpawner;
    use crate::api::Client as ApiClient;
    use crate::api::{ApiError, CompletionResponse, Provider};
    use crate::app::conversation::AssistantContent;
    use crate::app::domain::event::SessionEvent;
    use crate::app::domain::session::EventStore;
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
    use std::sync::Mutex as StdMutex;
    use steer_tools::tools::edit::multi_edit::MULTI_EDIT_TOOL_NAME;
    use steer_tools::tools::replace::REPLACE_TOOL_NAME;
    use steer_tools::tools::{
        BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME,
        VIEW_TOOL_NAME,
    };
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    #[derive(Clone)]
    struct RecordingProvider {
        response: String,
        last_system: Arc<StdMutex<Option<String>>>,
    }

    impl RecordingProvider {
        fn new(response: impl Into<String>, last_system: Arc<StdMutex<Option<String>>>) -> Self {
            Self {
                response: response.into(),
                last_system,
            }
        }
    }

    #[async_trait::async_trait]
    impl Provider for RecordingProvider {
        fn name(&self) -> &'static str {
            "recording"
        }

        async fn complete(
            &self,
            _model_id: &crate::config::model::ModelId,
            _messages: Vec<crate::app::conversation::Message>,
            system: Option<crate::app::SystemContext>,
            _tools: Option<Vec<steer_tools::ToolSchema>>,
            _call_options: Option<crate::config::model::ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            *self
                .last_system
                .lock()
                .expect("system prompt lock poisoned") = system.and_then(|context| context.render());

            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: self.response.clone(),
                }],
            })
        }
    }

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

    #[tokio::test]
    async fn sub_agent_persists_events_and_uses_whitelist_visibility() {
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

        let system_capture = Arc::new(StdMutex::new(None));
        let model = builtin::claude_sonnet_4_5();
        api_client.insert_test_provider(
            model.provider.clone(),
            Arc::new(RecordingProvider::new("ok", system_capture.clone())),
        );

        let spawner = DefaultAgentSpawner::new(
            event_store.clone(),
            api_client,
            workspace.clone(),
            model_registry,
            None,
            None,
        );

        let parent_session_id = crate::app::domain::types::SessionId::new();
        let allowed_tools = vec![
            VIEW_TOOL_NAME.to_string(),
            "mcp__alpha__allowed".to_string(),
        ];
        let system_prompt = "subagent system".to_string();

        let config = SubAgentConfig {
            parent_session_id,
            prompt: "hello".to_string(),
            allowed_tools: allowed_tools.clone(),
            model: model.clone(),
            system_context: Some(crate::app::SystemContext::new(system_prompt.clone())),
            workspace: Some(workspace),
            workspace_ref: None,
            workspace_id: None,
            repo_ref: None,
            workspace_name: None,
            mcp_backends: Vec::new(),
            allow_mcp_tools: true,
        };

        let result = spawner
            .spawn(config, CancellationToken::new())
            .await
            .expect("spawn sub-agent");

        let events = event_store
            .load_events(result.session_id)
            .await
            .expect("load events");

        let mut saw_session_created = false;
        let mut saw_assistant_message = false;
        let mut seen_visibility = None;
        let mut seen_preapproved = None;

        for (_, event) in events {
            match event {
                SessionEvent::SessionCreated { config, .. } => {
                    saw_session_created = true;
                    assert_eq!(config.parent_session_id, Some(parent_session_id));
                    let configured_system = config
                        .system_prompt
                        .as_deref()
                        .expect("expected system prompt in session config");
                    assert!(
                        configured_system.starts_with(system_prompt.as_str()),
                        "expected system prompt prefix, got: {configured_system:?}"
                    );
                    match &config.tool_config.visibility {
                        ToolVisibility::Whitelist(allowed) => {
                            seen_visibility = Some(allowed.clone());
                        }
                        other => panic!("expected whitelist visibility, got {other:?}"),
                    }
                    seen_preapproved =
                        Some(config.tool_config.approval_policy.preapproved.tools.clone());
                }
                SessionEvent::AssistantMessageAdded { .. } => {
                    saw_assistant_message = true;
                }
                _ => {}
            }
        }

        assert!(saw_session_created, "expected SessionCreated event");
        assert!(
            saw_assistant_message,
            "expected AssistantMessageAdded event"
        );

        let expected: HashSet<String> = allowed_tools.into_iter().collect();
        assert_eq!(seen_visibility, Some(expected.clone()));
        assert_eq!(seen_preapproved, Some(expected));

        let captured_system = system_capture
            .lock()
            .expect("system capture lock poisoned")
            .clone();
        let captured_system = captured_system.expect("expected captured system prompt");
        assert!(
            captured_system.starts_with(system_prompt.as_str()),
            "expected system prompt prefix, got: {captured_system:?}"
        );
    }
}
