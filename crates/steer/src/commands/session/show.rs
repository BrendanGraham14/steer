use async_trait::async_trait;
use chrono::{Local, TimeZone, Utc};
use eyre::{Result, eyre};
use uuid::Uuid;

use super::super::Command;

use steer_core::app::domain::session::{SessionCatalog, SqliteEventStore};
use steer_core::app::domain::types::SessionId;

pub struct ShowSessionCommand {
    pub session_id: String,
    pub remote: Option<String>,
    pub session_db: Option<std::path::PathBuf>,
}

#[async_trait]
impl Command for ShowSessionCommand {
    async fn execute(&self) -> Result<()> {
        if let Some(remote_addr) = &self.remote {
            return self.handle_remote(remote_addr).await;
        }

        let db_path = match &self.session_db {
            Some(path) => path.clone(),
            None => steer_core::utils::session::create_session_store_path()?,
        };

        let catalog = SqliteEventStore::new(&db_path)
            .await
            .map_err(|e| eyre!("Failed to open session database: {}", e))?;

        let session_id = Uuid::parse_str(&self.session_id)
            .map(SessionId::from)
            .map_err(|_| eyre!("Invalid session ID: {}", self.session_id))?;

        let summary = catalog
            .get_session_summary(session_id)
            .await
            .map_err(|e| eyre!("Failed to get session: {}", e))?;

        match summary {
            Some(info) => {
                println!("Session Details:");
                println!("ID: {}", info.id);
                println!(
                    "Created: {}",
                    info.created_at
                        .with_timezone(&Local)
                        .format("%Y-%m-%d %H:%M:%S")
                );
                println!(
                    "Updated: {}",
                    info.updated_at
                        .with_timezone(&Local)
                        .format("%Y-%m-%d %H:%M:%S")
                );
                println!("Messages: {}", info.message_count);
                println!(
                    "Last Model: {}",
                    info.last_model.unwrap_or_else(|| "N/A".to_string())
                );
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
                        Some(dt) => println!(
                            "Created: {}",
                            dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S")
                        ),
                        None => println!("Created: N/A"),
                    }
                }

                if let Some(updated_at) = state.updated_at {
                    let secs = updated_at.seconds;
                    let nsecs = updated_at.nanos as u32;
                    let datetime = Utc.timestamp_opt(secs, nsecs).single();
                    match datetime {
                        Some(dt) => println!(
                            "Updated: {}",
                            dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S")
                        ),
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
