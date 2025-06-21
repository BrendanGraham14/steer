use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use tokio::sync::mpsc;

use super::super::Command;
use conductor_core::api::Model;
use conductor_core::app::AppConfig;
use conductor_core::config::LlmConfig;
use conductor_core::events::StreamEventWithMetadata;
use conductor_core::session::{
    OrderDirection, SessionFilter, SessionManager, SessionManagerConfig, SessionOrderBy,
};
use conductor_core::utils::session::create_session_store;

pub struct ResumeSessionCommand {
    pub session_id: String,
    pub remote: Option<String>,
}

pub struct LatestSessionCommand {
    pub remote: Option<String>,
}

#[async_trait]
impl Command for ResumeSessionCommand {
    async fn execute(&self) -> Result<()> {
        // If remote is specified, handle via gRPC
        if let Some(remote_addr) = &self.remote {
            println!(
                "Resuming remote session: {} at {}",
                self.session_id, remote_addr
            );

            // TODO: The TUI functionality has been moved to conductor-tui crate
            // For now, just return an error
            return Err(anyhow!("Remote session resumption with TUI is not available in this command. Use the conductor-tui binary instead."));
        }

        // Local session handling
        println!("Resuming session: {}", self.session_id);

        let llm_config = LlmConfig::from_env()?;

        // Create session manager with SQLite store
        let session_store = create_session_store().await?;
        let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

        let session_manager_config = SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: Model::ClaudeSonnet4_20250514,
            auto_persist: true,
        };

        let session_manager =
            SessionManager::new(session_store, session_manager_config, global_event_tx);

        // Resume the session
        let app_config = AppConfig { llm_config };

        match session_manager
            .resume_session(&self.session_id, app_config)
            .await
        {
            Ok((true, command_tx)) => {
                // Get the event receiver
                let event_rx = session_manager
                    .take_event_receiver(&self.session_id)
                    .await
                    .map_err(|e| {
                        anyhow!("Failed to get event receiver for resumed session: {}", e)
                    })?;

                // Get the session state to restore messages
                let session_state = session_manager
                    .get_session_state(&self.session_id)
                    .await?
                    .ok_or_else(|| anyhow!("Session state not found after resume"))?;

                // Get the session info to determine the model
                let session_info = session_manager
                    .get_session(&self.session_id)
                    .await?
                    .ok_or_else(|| anyhow!("Session not found after resume"))?;

                let model = session_info
                    .last_model
                    .unwrap_or(Model::ClaudeSonnet4_20250514);

                // TODO: The TUI functionality has been moved to conductor-tui crate
                // For now, just print session info
                println!("Session resumed successfully. Session ID: {}", self.session_id);
                println!("Model: {}", model);
                println!("To interact with the session, use the conductor-tui binary.");
            }
            Ok((false, _)) => {
                return Err(anyhow!("Session {} not found", self.session_id));
            }
            Err(e) => {
                return Err(anyhow!("Failed to resume session: {}", e));
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Command for LatestSessionCommand {
    async fn execute(&self) -> Result<()> {
        // If remote is specified, handle via gRPC
        if let Some(remote_addr) = &self.remote {
            return self.handle_remote(remote_addr).await;
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

        // Get the most recently updated session
        let filter = SessionFilter {
            order_by: SessionOrderBy::UpdatedAt,
            order_direction: OrderDirection::Descending,
            limit: Some(1),
            ..Default::default()
        };

        let sessions = session_manager
            .list_sessions(filter)
            .await
            .map_err(|e| anyhow!("Failed to list sessions: {}", e))?;

        if sessions.is_empty() {
            return Err(anyhow!("No sessions found"));
        }

        let latest_session = &sessions[0];
        let session_id = &latest_session.id;

        println!("Resuming latest session: {}", session_id);
        println!(
            "Last updated: {}",
            latest_session.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
        );

        // Resume the session in the TUI directly
        let llm_config = LlmConfig::from_env()?;

        // Create session manager with SQLite store
        let session_store = create_session_store().await?;
        let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

        let session_manager_config = SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: Model::ClaudeSonnet4_20250514,
            auto_persist: true,
        };

        let session_manager =
            SessionManager::new(session_store, session_manager_config, global_event_tx);

        // Resume the session
        let app_config = AppConfig { llm_config };

        match session_manager.resume_session(session_id, app_config).await {
            Ok((true, command_tx)) => {
                // Get the event receiver
                let event_rx = session_manager
                    .take_event_receiver(session_id)
                    .await
                    .map_err(|e| {
                        anyhow!("Failed to get event receiver for resumed session: {}", e)
                    })?;

                // Get the session state to restore messages
                let session_state = session_manager
                    .get_session_state(session_id)
                    .await
                    .map_err(|e| anyhow!("Failed to get session state: {}", e))?
                    .ok_or_else(|| anyhow!("Session state not found after resume"))?;

                let model = latest_session
                    .last_model
                    .unwrap_or(Model::ClaudeSonnet4_20250514);

                // TODO: The TUI functionality has been moved to conductor-tui crate
                // For now, just print session info
                println!("Session created from snapshot successfully. Session ID: {}", session_id);
                println!("Model: {}", model);
                println!("To interact with the session, use the conductor-tui binary.");
            }
            Ok((false, _)) => {
                return Err(anyhow!("Session {} not found", session_id));
            }
            Err(e) => {
                return Err(anyhow!("Failed to resume session: {}", e));
            }
        }

        Ok(())
    }
}

impl LatestSessionCommand {
    async fn handle_remote(&self, remote_addr: &str) -> Result<()> {
        use conductor_grpc::GrpcClientAdapter;

        // Connect to the gRPC server
        let mut client = GrpcClientAdapter::connect(remote_addr).await.map_err(|e| {
            anyhow!(
                "Failed to connect to remote server at {}: {}",
                remote_addr,
                e
            )
        })?;

        // Get latest session via gRPC, then resume it
        let sessions = client
            .list_sessions()
            .await
            .map_err(|e| anyhow!("Failed to list remote sessions: {}", e))?;

        if sessions.is_empty() {
            return Err(anyhow!("No remote sessions found"));
        }

        // Find the most recently updated session
        let latest_session = sessions
            .into_iter()
            .max_by_key(|session| {
                session
                    .updated_at
                    .as_ref()
                    .and_then(|ts: &prost_types::Timestamp| {
                        let secs = ts.seconds;
                        let nsecs = ts.nanos as u32;
                        let datetime = Utc.timestamp_opt(secs, nsecs).single();
                        datetime.map(|dt| dt.timestamp())
                    })
                    .unwrap_or(0)
            })
            .ok_or_else(|| anyhow!("Failed to find latest session"))?;

        let session_id = latest_session.id;
        println!("Resuming latest remote session: {}", session_id);

        if let Some(updated_at) = latest_session.updated_at {
            let secs = updated_at.seconds;
            let nsecs = updated_at.nanos as u32;
            let datetime = Utc.timestamp_opt(secs, nsecs).single();
            match datetime {
                Some(dt) => println!("Last updated: {}", dt.format("%Y-%m-%d %H:%M:%S UTC")),
                None => println!("Last updated: N/A"),
            }
        }

        // TODO: The TUI functionality has been moved to conductor-tui crate
        // For now, just return an error
        return Err(anyhow!("Remote session resumption with TUI is not available in this command. Use the conductor-tui binary instead."));

        Ok(())
    }
}
