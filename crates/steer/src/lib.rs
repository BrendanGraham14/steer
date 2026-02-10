pub mod cli;
pub mod commands;
pub mod error;
pub mod model_resolver;
pub mod session_config;

pub use steer_core::{api, app, config, runners, session, tools, utils, workspace};

use eyre::Result;
use std::sync::Arc;
use steer_core::api::Client as ApiClient;
use steer_core::app::domain::runtime::{RuntimeHandle, RuntimeService};
use steer_core::app::domain::session::SqliteEventStore;
use steer_core::app::domain::types::SessionId;
use steer_core::config::model::ModelId;
use steer_core::runners::{OneShotRunner, RunOnceResult};
use steer_core::session::state::SessionConfig;
use steer_core::tools::ToolSystemBuilder;
use steer_core::utils::paths::AppPaths;
use steer_core::workspace::LocalWorkspaceManager;
use steer_core::workspace::RepoManager;

pub async fn run_once_in_session(
    runtime: &RuntimeHandle,
    session_id: SessionId,
    message: String,
    model: ModelId,
) -> Result<RunOnceResult> {
    OneShotRunner::run_in_session(runtime, session_id, message, model)
        .await
        .map_err(|e| eyre::eyre!("Failed to run in session: {}", e))
}

pub async fn run_once_new_session(
    runtime: &RuntimeHandle,
    config: SessionConfig,
    message: String,
    model: ModelId,
) -> Result<RunOnceResult> {
    OneShotRunner::run_new_session(runtime, config, message, model)
        .await
        .map_err(|e| eyre::eyre!("Failed to run new session: {}", e))
}

pub struct RuntimeBuilder {
    default_model: String,
    catalog_paths: Vec<String>,
}

impl RuntimeBuilder {
    pub fn new(default_model: String) -> Self {
        Self {
            default_model,
            catalog_paths: Vec::new(),
        }
    }

    pub fn with_catalogs(mut self, paths: Vec<String>) -> Self {
        self.catalog_paths = paths;
        self
    }

    pub async fn build(self) -> Result<(RuntimeService, ModelId)> {
        let event_store = create_event_store().await?;

        let auth_storage = Arc::new(
            steer_core::auth::DefaultAuthStorage::new()
                .map_err(|e| eyre::eyre!("Failed to create auth storage: {}", e))?,
        );

        let app_config = steer_core::app::AppConfig::from_auth_storage_with_catalog(
            auth_storage,
            steer_core::catalog::CatalogConfig::with_catalogs(self.catalog_paths),
        )
        .map_err(|e| eyre::eyre!("Failed to create app config: {}", e))?;

        let model_id = app_config
            .model_registry
            .resolve(&self.default_model)
            .map_err(|e| eyre::eyre!("Invalid model '{}': {}", self.default_model, e))?;

        let api_client = Arc::new(ApiClient::new_with_deps(
            app_config.llm_config_provider.clone(),
            app_config.provider_registry.clone(),
            app_config.model_registry.clone(),
        ));

        let workspace_root = std::env::current_dir()
            .map_err(|e| eyre::eyre!("Failed to get current directory: {}", e))?;
        let environment_root = AppPaths::local_environment_root();
        let workspace_config = steer_core::session::state::WorkspaceConfig::Local {
            path: workspace_root.clone(),
        };

        let workspace =
            steer_core::workspace::create_workspace_from_session_config(&workspace_config)
                .await
                .map_err(|e| eyre::eyre!("Failed to create workspace: {}", e))?;
        let workspace_manager = Arc::new(
            LocalWorkspaceManager::new(environment_root)
                .await
                .map_err(|e| eyre::eyre!("Failed to create workspace manager: {}", e))?,
        );
        let repo_manager: Arc<dyn RepoManager> = workspace_manager.clone();

        let tool_executor = ToolSystemBuilder::new(
            workspace,
            event_store.clone(),
            api_client.clone(),
            app_config.model_registry.clone(),
        )
        .with_workspace_manager(workspace_manager)
        .with_repo_manager(repo_manager)
        .build();

        let service = RuntimeService::spawn(event_store, api_client, tool_executor);

        Ok((service, model_id))
    }
}

async fn create_event_store() -> Result<Arc<SqliteEventStore>> {
    let data_dir = AppPaths::user_data_dir()
        .ok_or_else(|| eyre::eyre!("Failed to get user data directory"))?;

    std::fs::create_dir_all(&data_dir)
        .map_err(|e| eyre::eyre!("Failed to create data directory: {}", e))?;

    let db_path = data_dir.join("events.db");

    let store = SqliteEventStore::new(&db_path)
        .await
        .map_err(|e| eyre::eyre!("Failed to create event store: {}", e))?;

    Ok(Arc::new(store))
}

pub async fn create_runtime(default_model: String) -> Result<(RuntimeService, ModelId)> {
    RuntimeBuilder::new(default_model).build().await
}

pub async fn create_runtime_with_catalogs(
    default_model: String,
    catalog_paths: Vec<String>,
) -> Result<(RuntimeService, ModelId)> {
    RuntimeBuilder::new(default_model)
        .with_catalogs(catalog_paths)
        .build()
        .await
}
