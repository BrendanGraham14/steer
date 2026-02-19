use async_trait::async_trait;
use chrono::{Local, TimeZone, Utc};
use eyre::{Result, eyre};
use std::io::Write;
use uuid::Uuid;

use super::super::Command;

use steer_core::app::domain::session::{SessionMetadataStore, SqliteEventStore};
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
                let mut stdout = std::io::stdout();
                writeln!(stdout, "Session Details:")?;
                writeln!(stdout, "ID: {}", info.id)?;
                writeln!(
                    stdout,
                    "Created: {}",
                    info.created_at
                        .with_timezone(&Local)
                        .format("%Y-%m-%d %H:%M:%S")
                )?;
                writeln!(
                    stdout,
                    "Updated: {}",
                    info.updated_at
                        .with_timezone(&Local)
                        .format("%Y-%m-%d %H:%M:%S")
                )?;
                writeln!(stdout, "Messages: {}", info.message_count)?;
                writeln!(
                    stdout,
                    "Last Model: {}",
                    info.last_model.unwrap_or_else(|| "N/A".to_string())
                )?;
                writeln!(
                    stdout,
                    "Title: {}",
                    info.title.unwrap_or_else(|| "N/A".to_string())
                )?;
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
                let mut stdout = std::io::stdout();
                writeln!(stdout, "Remote Session Details:")?;
                writeln!(stdout, "ID: {}", state.id)?;

                if let Some(created_at) = state.created_at {
                    let secs = created_at.seconds;
                    let nsecs = created_at.nanos as u32;
                    let datetime = Utc.timestamp_opt(secs, nsecs).single();
                    if let Some(dt) = datetime {
                        writeln!(
                            stdout,
                            "Created: {}",
                            dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S")
                        )?;
                    } else {
                        writeln!(stdout, "Created: N/A")?;
                    }
                }

                if let Some(updated_at) = state.updated_at {
                    let secs = updated_at.seconds;
                    let nsecs = updated_at.nanos as u32;
                    let datetime = Utc.timestamp_opt(secs, nsecs).single();
                    if let Some(dt) = datetime {
                        writeln!(
                            stdout,
                            "Updated: {}",
                            dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S")
                        )?;
                    } else {
                        writeln!(stdout, "Updated: N/A")?;
                    }
                }

                writeln!(stdout, "Messages: {}", state.messages.len())?;
                writeln!(stdout, "Last Event Sequence: {}", state.last_event_sequence)?;
                writeln!(stdout, "Approved Tools: {:?}", state.approved_tools)?;
                writeln!(
                    stdout,
                    "Title: {}",
                    state
                        .config
                        .as_ref()
                        .and_then(|config| config.title.clone())
                        .unwrap_or_else(|| "N/A".to_string())
                )?;

                if !state.metadata.is_empty() {
                    writeln!(stdout, "Metadata:")?;
                    for (key, value) in &state.metadata {
                        writeln!(stdout, "  {key}: {value}")?;
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
