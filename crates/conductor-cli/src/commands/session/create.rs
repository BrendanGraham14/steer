use async_trait::async_trait;
use eyre::{Result, eyre};
use tokio::sync::mpsc;

use super::super::Command;
use crate::session_config::{SessionConfigLoader, SessionConfigOverrides};
use conductor_core::api::Model;
use conductor_core::app::AppConfig;
use conductor_core::config::LlmConfig;
use conductor_core::events::StreamEventWithMetadata;
use conductor_core::session::{SessionManager, SessionManagerConfig};
use conductor_core::utils::session::{
    create_session_store_with_config, resolve_session_store_config,
};

pub struct CreateSessionCommand {
    pub session_config: Option<std::path::PathBuf>,
    pub metadata: Option<String>,
    pub remote: Option<String>,
    pub system_prompt: Option<String>,
    pub session_db: Option<std::path::PathBuf>,
}

#[async_trait]
impl Command for CreateSessionCommand {
    async fn execute(&self) -> Result<()> {
        // Create the loader with optional config path
        let overrides = SessionConfigOverrides {
            system_prompt: self.system_prompt.clone(),
            metadata: self.metadata.clone(),
        };

        let loader =
            SessionConfigLoader::new(self.session_config.clone()).with_overrides(overrides);

        let session_config = loader.load().await?;

        // If remote is specified, handle via gRPC
        if let Some(remote_addr) = &self.remote {
            println!("Creating remote session at {remote_addr}");

            // TODO: The TUI functionality has been moved to conductor-tui crate
            // For now, just create the session without launching the TUI
            return Err(eyre!(
                "Remote session creation with TUI is not available in this command. Use the conductor-tui binary instead."
            ));
        }

        // Local session handling
        let store_config = resolve_session_store_config(self.session_db.clone())?;
        let session_store = create_session_store_with_config(store_config).await?;
        let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

        let session_manager_config = SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: Model::default(),
            auto_persist: true,
        };

        let session_manager =
            SessionManager::new(session_store, session_manager_config, global_event_tx);

        let app_config = AppConfig {
            llm_config: LlmConfig::from_env()
                .await
                .map_err(|e| eyre!("Failed to load LLM config: {}", e))?,
        };

        let (session_id, _) = session_manager
            .create_session(session_config, app_config)
            .await
            .map_err(|e| eyre!("Failed to create session: {}", e))?;

        println!("Created session: {}", session_id);
        Ok(())
    }
}
