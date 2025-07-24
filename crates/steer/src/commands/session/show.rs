use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use eyre::{Result, eyre};

use super::super::Command;
use steer_core::api::Model;
use steer_core::session::{SessionManager, SessionManagerConfig};
use steer_core::utils::session::{create_session_store_with_config, resolve_session_store_config};

pub struct ShowSessionCommand {
    pub session_id: String,
    pub remote: Option<String>,
    pub session_db: Option<std::path::PathBuf>,
}

#[async_trait]
impl Command for ShowSessionCommand {
    async fn execute(&self) -> Result<()> {
        // If remote is specified, handle via gRPC
        if let Some(remote_addr) = &self.remote {
            return self.handle_remote(remote_addr).await;
        }

        // Local session handling
        let store_config = resolve_session_store_config(self.session_db.clone())?;
        let session_store = create_session_store_with_config(store_config).await?;
        let session_manager_config = SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: Model::default(),
            auto_persist: true,
        };

        let session_manager = SessionManager::new(session_store, session_manager_config);

        let session_info = session_manager
            .get_session(&self.session_id)
            .await
            .map_err(|e| eyre!("Failed to get session: {}", e))?;

        match session_info {
            Some(info) => {
                println!("Session Details:");
                println!("ID: {}", info.id);
                println!(
                    "Created: {}",
                    info.created_at.format("%Y-%m-%d %H:%M:%S UTC")
                );
                println!(
                    "Updated: {}",
                    info.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
                );
                println!("Messages: {}", info.message_count);
                println!(
                    "Last Model: {}",
                    info.last_model
                        .map(|m| m.as_ref().to_string())
                        .unwrap_or_else(|| "N/A".to_string())
                );

                if !info.metadata.is_empty() {
                    println!("Metadata:");
                    for (key, value) in &info.metadata {
                        println!("  {key}: {value}");
                    }
                }
            }
            None => {
                return Err(eyre!("Session not found: {}", self.session_id));
            }
        }

        Ok(())
    }
}

impl ShowSessionCommand {
    async fn handle_remote(&self, remote_addr: &str) -> Result<()> {
        use steer_grpc::AgentClient;

        // Connect to the gRPC server
        let client = AgentClient::connect(remote_addr).await.map_err(|e| {
            eyre!(
                "Failed to connect to remote server at {}: {}",
                remote_addr,
                e
            )
        })?;

        let session_state = client
            .get_session(&self.session_id)
            .await
            .map_err(|e| eyre!("Failed to get remote session: {}", e))?;

        match session_state {
            Some(state) => {
                println!("Remote Session Details:");
                println!("ID: {}", state.id);

                if let Some(created_at) = state.created_at {
                    let secs = created_at.seconds;
                    let nsecs = created_at.nanos as u32;
                    let datetime = Utc.timestamp_opt(secs, nsecs).single();
                    match datetime {
                        Some(dt) => println!("Created: {}", dt.format("%Y-%m-%d %H:%M:%S UTC")),
                        None => println!("Created: N/A"),
                    }
                }

                if let Some(updated_at) = state.updated_at {
                    let secs = updated_at.seconds;
                    let nsecs = updated_at.nanos as u32;
                    let datetime = Utc.timestamp_opt(secs, nsecs).single();
                    match datetime {
                        Some(dt) => println!("Updated: {}", dt.format("%Y-%m-%d %H:%M:%S UTC")),
                        None => println!("Updated: N/A"),
                    }
                }

                println!("Messages: {}", state.messages.len());
                println!("Last Event Sequence: {}", state.last_event_sequence);
                println!("Approved Tools: {:?}", state.approved_tools);

                if !state.metadata.is_empty() {
                    println!("Metadata:");
                    for (key, value) in &state.metadata {
                        println!("  {key}: {value}");
                    }
                }
            }
            None => {
                return Err(eyre!("Remote session not found: {}", self.session_id));
            }
        }

        Ok(())
    }
}
