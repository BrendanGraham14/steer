use async_trait::async_trait;
use eyre::{Result, eyre};
use tracing::warn;

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
    pub preferred_model: Option<String>,
}

#[async_trait]
impl Command for CreateSessionCommand {
    async fn execute(&self) -> Result<()> {
        if self.system_prompt.is_some() {
            return Err(eyre!(
                "system_prompt is no longer supported; use primary agent policies instead"
            ));
        }

        let mut local_grpc_setup = None;
        let client = if let Some(remote_addr) = &self.remote {
            println!("Creating remote session at {remote_addr}");
            if !self.catalogs.is_empty() {
                tracing::warn!("Ignoring --catalog for remote session creation");
            }
            if self.session_db.is_some() {
                tracing::warn!("Ignoring --session-db for remote session creation");
            }
            AgentClient::connect(remote_addr)
                .await
                .map_err(|e| eyre!("Failed to connect to remote server: {}", e))?
        } else {
            let catalog_paths = self.normalize_catalog_paths();

            let db_path = match &self.session_db {
                Some(path) => path.clone(),
                None => steer_core::utils::session::create_session_store_path()?,
            };

            let catalog_config = CatalogConfig::with_catalogs(catalog_paths);

            let setup = steer_grpc::local_server::setup_local_grpc_with_catalog(
                steer_core::config::model::builtin::default_model(),
                Some(db_path),
                catalog_config,
                None,
            )
            .await
            .map_err(|e| eyre!("Failed to setup local gRPC: {}", e))?;

            let client = AgentClient::from_channel(setup.channel.clone())
                .await
                .map_err(|e| eyre!("Failed to create gRPC client: {}", e))?;
            local_grpc_setup = Some(setup);
            client
        };

        let server_default = client
            .get_default_model()
            .await
            .map_err(|e| eyre!("Failed to fetch server default model: {}", e))?;

        let model_input = self.model.as_deref().or(self.preferred_model.as_deref());
        let default_model = if let Some(input) = model_input {
            match client.resolve_model(input).await {
                Ok(model_id) => model_id,
                Err(e) => {
                    let fallback = format!(
                        "{}/{}",
                        server_default.provider.storage_key(),
                        server_default.id
                    );
                    warn!(
                        "Failed to resolve preferred model '{}': {}. Using server default {}.",
                        input, e, fallback
                    );
                    eprintln!(
                        "Warning: preferred model '{}' is invalid. Using server default {}.",
                        input, fallback
                    );
                    server_default.clone()
                }
            }
        } else {
            server_default.clone()
        };

        let overrides = SessionConfigOverrides {
            metadata: self.metadata.clone(),
            default_model: self.model.as_ref().map(|_| default_model.clone()),
            ..Default::default()
        };

        let loader = SessionConfigLoader::new(default_model, self.session_config.clone())
            .with_overrides(overrides);

        let session_config = loader.load().await?;

        let session_id = client
            .create_session(session_config)
            .await
            .map_err(|e| eyre!("Failed to create session: {}", e))?;

        if let Some(setup) = local_grpc_setup {
            setup.server_handle.abort();
            setup.runtime_service.shutdown().await;
        }

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
