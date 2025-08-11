use async_trait::async_trait;
use chrono::{Local, TimeZone, Utc};
use eyre::{Result, eyre};

use super::super::Command;

use steer_core::session::{SessionFilter, SessionManager, SessionManagerConfig, SessionStatus};

pub struct ListSessionCommand {
    pub active: bool,
    pub limit: Option<u32>,
    pub remote: Option<String>,
    pub session_db: Option<std::path::PathBuf>,
}

#[async_trait]
impl Command for ListSessionCommand {
    async fn execute(&self) -> Result<()> {
        // If remote is specified, handle via gRPC
        if let Some(_remote_addr) = &self.remote {
            return self.handle_remote().await;
        }

        // Local session handling
        let store_config =
            steer_core::utils::session::resolve_session_store_config(self.session_db.clone())?;
        let session_store =
            steer_core::utils::session::create_session_store_with_config(store_config).await?;
        let session_manager_config = SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: steer_core::config::model::builtin::opus(),
            auto_persist: true,
        };

        let session_manager = SessionManager::new(session_store, session_manager_config);

        let filter = SessionFilter {
            status_filter: if self.active {
                Some(SessionStatus::Active)
            } else {
                None
            },
            limit: self.limit,
            ..Default::default()
        };

        let sessions = session_manager
            .list_sessions(filter)
            .await
            .map_err(|e| eyre!("Failed to list sessions: {}", e))?;

        if sessions.is_empty() {
            println!("No sessions found.");
            return Ok(());
        }

        println!("Sessions:");
        println!(
            "{:<36} {:<20} {:<20} {:<10} {:<30}",
            "ID", "Created", "Updated", "Messages", "Last Model"
        );
        println!("{}", "-".repeat(106));

        for session in sessions {
            let model_str = session
                .last_model
                .map(|(provider, model)| format!("{provider}/{model}"))
                .unwrap_or_else(|| "N/A".to_string());

            println!(
                "{:<36} {:<20} {:<20} {:<10} {:<30}",
                session.id,
                session
                    .created_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S"),
                session
                    .updated_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S"),
                session.message_count,
                model_str,
            );
        }

        Ok(())
    }
}

impl ListSessionCommand {
    async fn handle_remote(&self) -> Result<()> {
        use steer_grpc::AgentClient;

        let remote_addr = self.remote.as_ref().unwrap();

        // Connect to the gRPC server
        let client = AgentClient::connect(remote_addr).await.map_err(|e| {
            eyre!(
                "Failed to connect to remote server at {}: {}",
                remote_addr,
                e
            )
        })?;

        // List remote sessions via gRPC
        let sessions = client
            .list_sessions()
            .await
            .map_err(|e| eyre!("Failed to list remote sessions: {}", e))?;

        if sessions.is_empty() {
            println!("No remote sessions found.");
            return Ok(());
        }

        println!("Remote Sessions:");
        println!(
            "{:<36} {:<20} {:<20} {:<10}",
            "ID", "Created", "Updated", "Status"
        );
        println!("{}", "-".repeat(86));

        for session in sessions {
            let created_str = session
                .created_at
                .as_ref()
                .map(|ts: &prost_types::Timestamp| {
                    let secs = ts.seconds;
                    let nsecs = ts.nanos as u32;
                    let datetime = Utc.timestamp_opt(secs, nsecs).single();
                    match datetime {
                        Some(dt) => dt
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string(),
                        None => "N/A".to_string(),
                    }
                })
                .unwrap_or_else(|| "N/A".to_string());

            let updated_str = session
                .updated_at
                .as_ref()
                .map(|ts: &prost_types::Timestamp| {
                    let secs = ts.seconds;
                    let nsecs = ts.nanos as u32;
                    let datetime = Utc.timestamp_opt(secs, nsecs).single();
                    match datetime {
                        Some(dt) => dt
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string(),
                        None => "N/A".to_string(),
                    }
                })
                .unwrap_or_else(|| "N/A".to_string());

            let status_str = match session.status {
                0 => "Unspecified",
                1 => "Active",
                2 => "Inactive",
                _ => "Unknown",
            };

            println!(
                "{:<36} {:<20} {:<20} {:<10}",
                session.id, created_str, updated_str, status_str,
            );
        }

        Ok(())
    }
}
