use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::mpsc;

use super::super::Command;
use conductor_core::api::Model;
use conductor_core::app::AppConfig;
use conductor_core::config::LlmConfig;
use conductor_core::events::StreamEventWithMetadata;
use conductor_core::session::{
    SessionConfig, SessionManager, SessionManagerConfig, SessionToolConfig, WorkspaceConfig,
};
use conductor_core::utils::session::{create_session_store, parse_metadata, parse_tool_policy};

pub struct CreateSessionCommand {
    pub tool_policy: String,
    pub pre_approved_tools: Option<String>,
    pub metadata: Option<String>,
    pub remote: Option<String>,
    pub system_prompt: Option<String>,
}

#[async_trait]
impl Command for CreateSessionCommand {
    async fn execute(&self) -> Result<()> {
        let policy = parse_tool_policy(&self.tool_policy, self.pre_approved_tools.as_deref())?;
        let session_metadata = parse_metadata(self.metadata.as_deref())?;

        // TODO: Allow customizing from CLI args, a file, and/or env vars
        let mut tool_config = SessionToolConfig::default();
        tool_config.approval_policy = policy;

        let session_config = SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config,
            system_prompt: self.system_prompt.clone(),
            metadata: session_metadata,
        };

        // If remote is specified, handle via gRPC
        if let Some(remote_addr) = &self.remote {
            println!("Creating remote session at {}", remote_addr);

            // TODO: The TUI functionality has been moved to conductor-tui crate
            // For now, just create the session without launching the TUI
            return Err(anyhow!(
                "Remote session creation with TUI is not available in this command. Use the conductor-tui binary instead."
            ));
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
