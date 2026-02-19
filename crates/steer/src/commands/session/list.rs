use async_trait::async_trait;
use chrono::{Local, TimeZone, Utc};
use eyre::{Result, eyre};
use std::io::Write;

use super::super::Command;

use steer_core::app::domain::session::{SessionFilter, SessionMetadataStore, SqliteEventStore};

pub struct ListSessionCommand {
    pub active: bool,
    pub limit: Option<u32>,
    pub remote: Option<String>,
    pub session_db: Option<std::path::PathBuf>,
}

#[async_trait]
impl Command for ListSessionCommand {
    async fn execute(&self) -> Result<()> {
        if let Some(_remote_addr) = &self.remote {
            return self.handle_remote().await;
        }

        let db_path = match &self.session_db {
            Some(path) => path.clone(),
            None => steer_core::utils::session::create_session_store_path()?,
        };

        let catalog = SqliteEventStore::new(&db_path)
            .await
            .map_err(|e| eyre!("Failed to open session database: {}", e))?;

        let filter = SessionFilter {
            limit: self.limit.map(|l| l as usize),
            offset: None,
        };

        let sessions = catalog
            .list_sessions(filter)
            .await
            .map_err(|e| eyre!("Failed to list sessions: {}", e))?;

        if sessions.is_empty() {
            let mut stdout = std::io::stdout();
            writeln!(stdout, "No sessions found.")?;
            return Ok(());
        }

        let mut stdout = std::io::stdout();
        writeln!(stdout, "Sessions:")?;
        writeln!(
            stdout,
            "{:<36} {:<20} {:<20} {:<10} {:<30} {:<30}",
            "ID", "Created", "Updated", "Messages", "Last Model", "Title"
        )?;
        writeln!(stdout, "{}", "-".repeat(147))?;

        for session in sessions {
            let model_str = session.last_model.unwrap_or_else(|| "N/A".to_string());
            let title_str = session.title.unwrap_or_else(|| "N/A".to_string());

            writeln!(
                stdout,
                "{:<36} {:<20} {:<20} {:<10} {:<30} {:<30}",
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
                title_str,
            )?;
        }

        Ok(())
    }
}

impl ListSessionCommand {
    async fn handle_remote(&self) -> Result<()> {
        use steer_grpc::AgentClient;

        let remote_addr = match self.remote.as_ref() {
            Some(remote_addr) => remote_addr,
            None => return Ok(()),
        };

        let client = AgentClient::connect(remote_addr).await.map_err(|e| {
            eyre!(
                "Failed to connect to remote server at {}: {}",
                remote_addr,
                e
            )
        })?;

        let sessions = client
            .list_sessions()
            .await
            .map_err(|e| eyre!("Failed to list remote sessions: {}", e))?;

        if sessions.is_empty() {
            let mut stdout = std::io::stdout();
            writeln!(stdout, "No remote sessions found.")?;
            return Ok(());
        }

        let mut stdout = std::io::stdout();
        writeln!(stdout, "Remote Sessions:")?;
        writeln!(
            stdout,
            "{:<36} {:<20} {:<20} {:<10} {:<30}",
            "ID", "Created", "Updated", "Status", "Title"
        )?;
        writeln!(stdout, "{}", "-".repeat(118))?;

        for session in sessions {
            let created_str = session.created_at.as_ref().map_or_else(
                || "N/A".to_string(),
                |ts: &prost_types::Timestamp| {
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
                },
            );

            let updated_str = session.updated_at.as_ref().map_or_else(
                || "N/A".to_string(),
                |ts: &prost_types::Timestamp| {
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
                },
            );

            let status_str = match session.status {
                0 => "Unspecified",
                1 => "Active",
                2 => "Inactive",
                _ => "Unknown",
            };

            let title_str = session.title.unwrap_or_else(|| "N/A".to_string());

            writeln!(
                stdout,
                "{:<36} {:<20} {:<20} {:<10} {:<30}",
                session.id, created_str, updated_str, status_str, title_str,
            )?;
        }

        Ok(())
    }
}
