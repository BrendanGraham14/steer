use async_trait::async_trait;
use eyre::{Result, eyre};
use std::sync::Arc;

use super::super::Command;
use crate::session_config::{SessionConfigLoader, SessionConfigOverrides};

use steer_core::api::Client as ApiClient;
use steer_core::app::domain::runtime::RuntimeService;
use steer_core::app::domain::session::SqliteEventStore;
use steer_core::tools::ToolSystemBuilder;

pub struct CreateSessionCommand {
    pub session_config: Option<std::path::PathBuf>,
    pub metadata: Option<String>,
    pub remote: Option<String>,
    pub system_prompt: Option<String>,
    pub session_db: Option<std::path::PathBuf>,
    pub model: Option<String>,
}

#[async_trait]
impl Command for CreateSessionCommand {
    async fn execute(&self) -> Result<()> {
        let overrides = SessionConfigOverrides {
            system_prompt: self.system_prompt.clone(),
            metadata: self.metadata.clone(),
        };

        let default_model = if let Some(model_str) = &self.model {
            let registry = steer_core::model_registry::ModelRegistry::load(&[])
                .map_err(|e| eyre!("Failed to load model registry: {}", e))?;
            registry
                .resolve(model_str)
                .map_err(|e| eyre!("Failed to resolve model '{}': {}", model_str, e))?
        } else {
            steer_core::config::model::builtin::opus()
        };

        let loader = SessionConfigLoader::new(default_model, self.session_config.clone())
            .with_overrides(overrides);

        let session_config = loader.load().await?;

        if let Some(remote_addr) = &self.remote {
            println!("Creating remote session at {remote_addr}");

            return Err(eyre!(
                "Remote session creation with TUI is not available in this command. Use the steer-tui binary instead."
            ));
        }

        let db_path = match &self.session_db {
            Some(path) => path.clone(),
            None => steer_core::utils::session::create_session_store_path()?,
        };

        let event_store = Arc::new(
            SqliteEventStore::new(&db_path)
                .await
                .map_err(|e| eyre!("Failed to open session database: {}", e))?,
        );

        let auth_storage = Arc::new(
            steer_core::auth::DefaultAuthStorage::new()
                .map_err(|e| eyre!("Failed to create auth storage: {}", e))?,
        );

        let model_registry = Arc::new(
            steer_core::model_registry::ModelRegistry::load(&[])
                .map_err(|e| eyre!("Failed to load model registry: {}", e))?,
        );

        let provider_registry = Arc::new(
            steer_core::auth::ProviderRegistry::load(&[])
                .map_err(|e| eyre!("Failed to load provider registry: {}", e))?,
        );

        let llm_config_provider = steer_core::config::LlmConfigProvider::new(auth_storage);

        let api_client = Arc::new(ApiClient::new_with_deps(
            llm_config_provider,
            provider_registry,
            model_registry.clone(),
        ));

        let workspace = steer_core::workspace::create_workspace(
            &steer_core::workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            },
        )
        .await
        .map_err(|e| eyre!("Failed to create workspace: {}", e))?;

        let tool_executor = ToolSystemBuilder::new(
            workspace,
            event_store.clone(),
            api_client.clone(),
            model_registry.clone(),
        )
        .build();

        let runtime_service = RuntimeService::spawn(event_store, api_client, tool_executor);

        let session_id = runtime_service
            .handle()
            .create_session(session_config)
            .await
            .map_err(|e| eyre!("Failed to create session: {}", e))?;

        runtime_service.shutdown().await;

        println!("Created session: {session_id}");
        Ok(())
    }
}
