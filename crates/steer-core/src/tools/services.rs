use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::SystemContext;
use crate::app::conversation::Message;
use crate::app::domain::session::EventStore;
use crate::app::domain::types::SessionId;
use crate::config::model::ModelId;
use crate::session::state::BackendConfig;
use crate::workspace::{RepoManager, RepoRef, Workspace, WorkspaceId, WorkspaceManager, WorkspaceRef};

use super::capability::Capabilities;
use steer_tools::ToolSchema;

#[async_trait]
pub trait AgentSpawner: Send + Sync {
    async fn spawn(
        &self,
        config: SubAgentConfig,
        cancel_token: CancellationToken,
    ) -> Result<SubAgentResult, SubAgentError>;
}

#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    pub parent_session_id: SessionId,
    pub prompt: String,
    pub allowed_tools: Vec<String>,
    pub model: ModelId,
    pub system_context: Option<SystemContext>,
    pub workspace: Option<Arc<dyn Workspace>>,
    pub workspace_ref: Option<WorkspaceRef>,
    pub workspace_id: Option<WorkspaceId>,
    pub repo_ref: Option<RepoRef>,
    pub workspace_name: Option<String>,
    pub mcp_backends: Vec<BackendConfig>,
    pub allow_mcp_tools: bool,
}

#[derive(Debug, Clone)]
pub struct SubAgentResult {
    pub session_id: SessionId,
    pub final_message: Message,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SubAgentError {
    #[error("API error: {0}")]
    Api(String),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("Event store error: {0}")]
    EventStore(String),

    #[error("Cancelled")]
    Cancelled,
}

#[async_trait]
pub trait ModelCaller: Send + Sync {
    async fn call(
        &self,
        model: &ModelId,
        messages: Vec<Message>,
        system_context: Option<SystemContext>,
        cancel_token: CancellationToken,
    ) -> Result<Message, ModelCallError>;
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ModelCallError {
    #[error("API error: {0}")]
    Api(String),

    #[error("Cancelled")]
    Cancelled,
}

pub struct ToolServices {
    pub workspace: Arc<dyn Workspace>,
    pub event_store: Arc<dyn EventStore>,
    pub api_client: Arc<ApiClient>,

    agent_spawner: Option<Arc<dyn AgentSpawner>>,
    model_caller: Option<Arc<dyn ModelCaller>>,
    workspace_manager: Option<Arc<dyn WorkspaceManager>>,
    repo_manager: Option<Arc<dyn RepoManager>>,

    available_capabilities: Capabilities,
}

impl ToolServices {
    pub fn new(
        workspace: Arc<dyn Workspace>,
        event_store: Arc<dyn EventStore>,
        api_client: Arc<ApiClient>,
    ) -> Self {
        Self {
            workspace,
            event_store,
            api_client,
            agent_spawner: None,
            model_caller: None,
            workspace_manager: None,
            repo_manager: None,
            available_capabilities: Capabilities::WORKSPACE,
        }
    }

    pub fn with_agent_spawner(mut self, spawner: Arc<dyn AgentSpawner>) -> Self {
        self.agent_spawner = Some(spawner);
        self.available_capabilities |= Capabilities::AGENT_SPAWNER;
        self
    }

    pub fn with_model_caller(mut self, caller: Arc<dyn ModelCaller>) -> Self {
        self.model_caller = Some(caller);
        self.available_capabilities |= Capabilities::MODEL_CALLER;
        self
    }

    pub fn with_workspace_manager(mut self, manager: Arc<dyn WorkspaceManager>) -> Self {
        self.workspace_manager = Some(manager);
        self
    }

    pub fn with_repo_manager(mut self, manager: Arc<dyn RepoManager>) -> Self {
        self.repo_manager = Some(manager);
        self
    }

    pub fn with_network(mut self) -> Self {
        self.available_capabilities |= Capabilities::NETWORK;
        self
    }

    pub fn capabilities(&self) -> Capabilities {
        self.available_capabilities
    }

    pub fn agent_spawner(&self) -> Option<&Arc<dyn AgentSpawner>> {
        self.agent_spawner.as_ref()
    }

    pub fn model_caller(&self) -> Option<&Arc<dyn ModelCaller>> {
        self.model_caller.as_ref()
    }

    pub fn workspace_manager(&self) -> Option<&Arc<dyn WorkspaceManager>> {
        self.workspace_manager.as_ref()
    }

    pub fn repo_manager(&self) -> Option<&Arc<dyn RepoManager>> {
        self.repo_manager.as_ref()
    }
}

impl std::fmt::Debug for ToolServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolServices")
            .field("capabilities", &self.available_capabilities)
            .finish_non_exhaustive()
    }
}

pub fn filter_schemas_by_capabilities<'a>(
    schemas: impl Iterator<Item = (&'a ToolSchema, Capabilities)>,
    available: Capabilities,
) -> Vec<ToolSchema> {
    schemas
        .filter(|(_, required)| available.satisfies(*required))
        .map(|(schema, _)| schema.clone())
        .collect()
}
