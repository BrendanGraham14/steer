use async_trait::async_trait;
use eyre::{Result, eyre};

use super::super::Command;
use crate::session_config::{SessionConfigLoader, SessionConfigOverrides};

use steer_core::catalog::CatalogConfig;
use steer_grpc::AgentClient;

pub struct CreateSessionCommand {
    pub session_config: Option<std::path::PathBuf>,
    pub metadata: Option<String>,
    pub remote: Option<String>,
    pub system_prompt: Option<String>,
    pub session_db: Option<std::path::PathBuf>,
    pub model: Option<String>,
    pub catalogs: Vec<std::path::PathBuf>,
}

#[async_trait]
impl Command for CreateSessionCommand {
    async fn execute(&self) -> Result<()> {
        let overrides = SessionConfigOverrides {
            system_prompt: self.system_prompt.clone(),
            metadata: self.metadata.clone(),
        };

        if let Some(remote_addr) = &self.remote {
            println!("Creating remote session at {remote_addr}");

            return Err(eyre!(
                "Remote session creation with TUI is not available in this command. Use the steer-tui binary instead."
            ));
        }

        let catalog_paths = self.normalize_catalog_paths();

        let db_path = match &self.session_db {
            Some(path) => path.clone(),
            None => steer_core::utils::session::create_session_store_path()?,
        };

        let catalog_config = CatalogConfig::with_catalogs(catalog_paths);

        let local_grpc_setup = steer_grpc::local_server::setup_local_grpc_with_catalog(
            steer_core::config::model::builtin::opus(),
            Some(db_path),
            catalog_config,
        )
        .await
        .map_err(|e| eyre!("Failed to setup local gRPC: {}", e))?;

        let client = AgentClient::from_channel(local_grpc_setup.channel)
            .await
            .map_err(|e| eyre!("Failed to create gRPC client: {}", e))?;

        let model_input = self.model.as_deref().unwrap_or("opus");
        let default_model = client
            .resolve_model(model_input)
            .await
            .map_err(|e| eyre!("Failed to resolve model '{}': {}", model_input, e))?;

        let loader = SessionConfigLoader::new(default_model, self.session_config.clone())
            .with_overrides(overrides);

        let session_config = loader.load().await?;

        let session_id = client
            .create_session(session_config)
            .await
            .map_err(|e| eyre!("Failed to create session: {}", e))?;

        local_grpc_setup.server_handle.abort();
        local_grpc_setup.runtime_service.shutdown().await;

        println!("Created session: {session_id}");
        Ok(())
    }
}

impl CreateSessionCommand {
    fn normalize_catalog_paths(&self) -> Vec<String> {
        self.catalogs
            .iter()
            .map(|p| {
                if !p.exists() {
                    tracing::warn!("Catalog path does not exist: {}", p.display());
                    p.to_string_lossy().to_string()
                } else {
                    p.canonicalize()
                        .map(|c| c.to_string_lossy().to_string())
                        .unwrap_or_else(|_| p.to_string_lossy().to_string())
                }
            })
            .collect()
    }
}
