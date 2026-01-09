use std::sync::Arc;

use crate::api::Client as ApiClient;
use crate::app::domain::session::EventStore;
use crate::app::validation::ValidatorRegistry;
use crate::model_registry::ModelRegistry;
use crate::workspace::{Workspace, WorkspaceManager};

use super::BackendRegistry;
use super::agent_spawner_impl::DefaultAgentSpawner;
use super::executor::ToolExecutor;
use super::model_caller_impl::DefaultModelCaller;
use super::registry::ToolRegistry;
use super::services::ToolServices;
use super::static_tools::{
    AstGrepTool, BashTool, DispatchAgentTool, EditTool, FetchTool, GlobTool, GrepTool, LsTool,
    MultiEditTool, ReplaceTool, TodoReadTool, TodoWriteTool, ViewTool,
};

pub struct ToolSystemBuilder {
    workspace: Arc<dyn Workspace>,
    event_store: Arc<dyn EventStore>,
    api_client: Arc<ApiClient>,
    model_registry: Arc<ModelRegistry>,
    backend_registry: Arc<BackendRegistry>,
    validators: Arc<ValidatorRegistry>,
    workspace_manager: Option<Arc<dyn WorkspaceManager>>,
}

impl ToolSystemBuilder {
    pub fn new(
        workspace: Arc<dyn Workspace>,
        event_store: Arc<dyn EventStore>,
        api_client: Arc<ApiClient>,
        model_registry: Arc<ModelRegistry>,
    ) -> Self {
        Self {
            workspace,
            event_store,
            api_client,
            model_registry,
            backend_registry: Arc::new(BackendRegistry::new()),
            validators: Arc::new(ValidatorRegistry::new()),
            workspace_manager: None,
        }
    }

    pub fn with_backend_registry(mut self, registry: Arc<BackendRegistry>) -> Self {
        self.backend_registry = registry;
        self
    }

    pub fn with_validators(mut self, validators: Arc<ValidatorRegistry>) -> Self {
        self.validators = validators;
        self
    }

    pub fn with_workspace_manager(mut self, manager: Arc<dyn WorkspaceManager>) -> Self {
        self.workspace_manager = Some(manager);
        self
    }

    pub fn build(self) -> Arc<ToolExecutor> {
        let base_executor = ToolExecutor::with_components(
            self.workspace.clone(),
            self.backend_registry,
            self.validators,
        );

        let agent_spawner = Arc::new(DefaultAgentSpawner::new(
            self.event_store.clone(),
            self.api_client.clone(),
            self.workspace.clone(),
            self.model_registry,
        ));

        let model_caller = Arc::new(DefaultModelCaller::new(self.api_client.clone()));

        let mut services =
            ToolServices::new(self.workspace.clone(), self.event_store, self.api_client)
                .with_agent_spawner(agent_spawner)
                .with_model_caller(model_caller)
                .with_network();
        if let Some(manager) = self.workspace_manager {
            services = services.with_workspace_manager(manager);
        }
        let services = Arc::new(services);

        let mut registry = ToolRegistry::new();

        registry.register_static(GrepTool);
        registry.register_static(GlobTool);
        registry.register_static(LsTool);
        registry.register_static(ViewTool);
        registry.register_static(BashTool);
        registry.register_static(EditTool);
        registry.register_static(MultiEditTool);
        registry.register_static(ReplaceTool);
        registry.register_static(AstGrepTool);
        registry.register_static(TodoReadTool);
        registry.register_static(TodoWriteTool);
        registry.register_static(DispatchAgentTool);
        registry.register_static(FetchTool);

        Arc::new(base_executor.with_static_tools(Arc::new(registry), services))
    }
}
