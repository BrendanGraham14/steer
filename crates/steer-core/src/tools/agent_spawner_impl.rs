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
    ApprovalRules, SessionConfig, SessionToolConfig, ToolApprovalPolicy, ToolVisibility,
    UnapprovedBehavior, WorkspaceConfig,
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

        let approval_policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Prompt,
            preapproved: ApprovalRules {
                tools: visibility_tools.clone(),
                per_tool: HashMap::new(),
            },
        };

        let tool_config = SessionToolConfig {
            backends: mcp_backends,
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
        session_config.system_prompt = config.system_prompt.clone();
        session_config.tool_config = tool_config;

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
