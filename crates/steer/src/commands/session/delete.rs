use async_trait::async_trait;
use eyre::{Result, eyre};
use std::io::{self, Write};
use uuid::Uuid;

fn confirm_prompt(prompt: &str) -> Result<bool> {
    let mut stdout = io::stdout();
    stdout.write_all(prompt.as_bytes())?;
    stdout.flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input.trim().to_lowercase().starts_with('y'))
}

use super::super::Command;

use steer_core::app::domain::session::{EventStore, SqliteEventStore};
use steer_core::app::domain::types::SessionId;

pub struct DeleteSessionCommand {
    pub session_id: String,
    pub force: bool,
    pub remote: Option<String>,
    pub session_db: Option<std::path::PathBuf>,
}

#[async_trait]
impl Command for DeleteSessionCommand {
    async fn execute(&self) -> Result<()> {
        if let Some(remote_addr) = &self.remote {
            return self.handle_remote(remote_addr).await;
        }

        if !self.force
            && !confirm_prompt(&format!(
                "Are you sure you want to delete session {}? (y/N): ",
                self.session_id
            ))?
        {
            let mut stdout = io::stdout();
            writeln!(stdout, "Deletion cancelled.")?;
            return Ok(());
        }

        let db_path = match &self.session_db {
            Some(path) => path.clone(),
            None => steer_core::utils::session::create_session_store_path()?,
        };

        let event_store = SqliteEventStore::new(&db_path)
            .await
            .map_err(|e| eyre!("Failed to open session database: {}", e))?;

        let session_id = Uuid::parse_str(&self.session_id)
            .map(SessionId::from)
            .map_err(|_| eyre!("Invalid session ID: {}", self.session_id))?;

        let exists = event_store
            .session_exists(session_id)
            .await
            .map_err(|e| eyre!("Failed to check session: {}", e))?;

        if !exists {
            return Err(eyre!("Session not found: {}", self.session_id));
        }

        event_store
            .delete_session(session_id)
            .await
            .map_err(|e| eyre!("Failed to delete session: {}", e))?;

        let mut stdout = io::stdout();
        writeln!(stdout, "Session {} deleted.", self.session_id)?;
        Ok(())
    }
}

impl DeleteSessionCommand {
    async fn handle_remote(&self, remote_addr: &str) -> Result<()> {
        use steer_grpc::AgentClient;

        let client = AgentClient::connect(remote_addr).await.map_err(|e| {
            eyre!(
                "Failed to connect to remote server at {}: {}",
                remote_addr,
                e
            )
        })?;

        if !self.force
            && !confirm_prompt(&format!(
                "Are you sure you want to delete remote session {}? (y/N): ",
                self.session_id
            ))?
        {
            let mut stdout = io::stdout();
            writeln!(stdout, "Deletion cancelled.")?;
            return Ok(());
        }

        let deleted = client
            .delete_session(&self.session_id)
            .await
            .map_err(|e| eyre!("Failed to delete remote session: {}", e))?;

        if deleted {
            let mut stdout = io::stdout();
            writeln!(stdout, "Remote session {} deleted.", self.session_id)?;
        } else {
            return Err(eyre!("Remote session not found: {}", self.session_id));
        }

        Ok(())
    }
}
