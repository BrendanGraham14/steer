use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::io::{self, Write};
use tokio::sync::mpsc;

use super::super::Command;
use conductor_core::api::Model;
use conductor_core::events::StreamEventWithMetadata;
use conductor_core::session::{SessionManager, SessionManagerConfig};
use conductor_core::utils::session::create_session_store;

pub struct DeleteSessionCommand {
    pub session_id: String,
    pub force: bool,
    pub remote: Option<String>,
}

#[async_trait]
impl Command for DeleteSessionCommand {
    async fn execute(&self) -> Result<()> {
        // If remote is specified, handle via gRPC
        if let Some(remote_addr) = &self.remote {
            return self.handle_remote(remote_addr).await;
        }

        // Local session handling
        if !self.force {
            print!(
                "Are you sure you want to delete session {}? (y/N): ",
                self.session_id
            );
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            if !input.trim().to_lowercase().starts_with('y') {
                println!("Deletion cancelled.");
                return Ok(());
            }
        }

        let session_store = create_session_store().await?;
        let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

        let session_manager_config = SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: Model::ClaudeSonnet4_20250514,
            auto_persist: true,
        };

        let session_manager =
            SessionManager::new(session_store, session_manager_config, global_event_tx);

        let deleted = session_manager
            .delete_session(&self.session_id)
            .await
            .map_err(|e| anyhow!("Failed to delete session: {}", e))?;

        if deleted {
            println!("Session {} deleted.", self.session_id);
        } else {
            return Err(anyhow!("Session not found: {}", self.session_id));
        }

        Ok(())
    }
}

impl DeleteSessionCommand {
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

        if !self.force {
            print!(
                "Are you sure you want to delete remote session {}? (y/N): ",
                self.session_id
            );
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            if !input.trim().to_lowercase().starts_with('y') {
                println!("Deletion cancelled.");
                return Ok(());
            }
        }

        let deleted = client
            .delete_session(&self.session_id)
            .await
            .map_err(|e| anyhow!("Failed to delete remote session: {}", e))?;

        if deleted {
            println!("Remote session {} deleted.", self.session_id);
        } else {
            return Err(anyhow!("Remote session not found: {}", self.session_id));
        }

        Ok(())
    }
}
