use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::mpsc;

use super::super::Command;
use crate::api::Model;
use crate::app::AppConfig;
use crate::config::LlmConfig;
use crate::events::StreamEventWithMetadata;
use crate::session::{SessionConfig, SessionManager, SessionManagerConfig, SessionToolConfig};
use crate::utils::session::{create_session_store, parse_metadata, parse_tool_policy};

pub struct CreateSessionCommand {
    pub tool_policy: String,
    pub pre_approved_tools: Option<String>,
    pub metadata: Option<String>,
    pub remote: Option<String>,
}

#[async_trait]
impl Command for CreateSessionCommand {
    async fn execute(&self) -> Result<()> {
        let policy = parse_tool_policy(&self.tool_policy, self.pre_approved_tools.as_deref())?;
        let session_metadata = parse_metadata(self.metadata.as_deref())?;

        let session_config = SessionConfig {
            tool_policy: policy,
            tool_config: SessionToolConfig::default(),
            metadata: session_metadata,
        };

        // If remote is specified, handle via gRPC
        if let Some(remote_addr) = &self.remote {
            println!("Creating remote session at {}", remote_addr);

            // Set panic hook for terminal cleanup
            crate::tui::setup_panic_hook();

            // Create TUI in remote mode with custom session config
            let (mut tui, event_rx) = crate::tui::Tui::new_remote(
                remote_addr,
                Model::ClaudeSonnet4_20250514, // Default model, could be made configurable
                Some(session_config),
            )
            .await?;

            println!("Connected to remote server and created session");

            // Run the TUI with events from the remote server
            tui.run(event_rx).await?;
            return Ok(());
        }

        // Local session handling
        let session_store = create_session_store().await?;
        let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

        let session_manager_config = SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: Model::ClaudeSonnet4_20250514,
            auto_persist: true,
        };

        let session_manager =
            SessionManager::new(session_store, session_manager_config, global_event_tx);

        let app_config = AppConfig {
            llm_config: LlmConfig::from_env()?,
        };

        let (session_id, _) = session_manager
            .create_session(session_config, app_config)
            .await
            .map_err(|e| anyhow!("Failed to create session: {}", e))?;

        println!("Created session: {}", session_id);
        Ok(())
    }
}
