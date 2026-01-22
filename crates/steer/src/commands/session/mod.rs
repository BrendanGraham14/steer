use async_trait::async_trait;
use eyre::Result;

use super::Command;
use crate::cli::SessionCommands;

mod create;
mod delete;
mod list;
mod show;

pub use create::CreateSessionCommand;
pub use delete::DeleteSessionCommand;
pub use list::ListSessionCommand;
pub use show::ShowSessionCommand;

pub struct SessionCommand {
    pub command: SessionCommands,
    pub remote: Option<String>,
    pub session_db: Option<std::path::PathBuf>,
    pub catalogs: Vec<std::path::PathBuf>,
    pub preferred_model: Option<String>,
}

#[async_trait]
impl Command for SessionCommand {
    async fn execute(&self) -> Result<()> {
        // Dispatch to appropriate subcommand
        match &self.command {
            SessionCommands::List { active, limit } => {
                let cmd = ListSessionCommand {
                    active: *active,
                    limit: Some(*limit),
                    remote: self.remote.clone(),
                    session_db: self.session_db.clone(),
                };
                cmd.execute().await
            }
            SessionCommands::Create {
                session_config,
                metadata,
                system_prompt,
                model,
            } => {
                let cmd = CreateSessionCommand {
                    session_config: session_config.clone(),
                    metadata: metadata.clone(),
                    remote: self.remote.clone(),
                    system_prompt: system_prompt.clone(),
                    session_db: self.session_db.clone(),
                    model: model.clone(),
                    catalogs: self.catalogs.clone(),
                    preferred_model: self.preferred_model.clone(),
                };
                cmd.execute().await
            }
            SessionCommands::Delete { session_id, force } => {
                let cmd = DeleteSessionCommand {
                    session_id: session_id.clone(),
                    force: *force,
                    remote: self.remote.clone(),
                    session_db: self.session_db.clone(),
                };
                cmd.execute().await
            }
            SessionCommands::Show { session_id } => {
                let cmd = ShowSessionCommand {
                    session_id: session_id.clone(),
                    remote: self.remote.clone(),
                    session_db: self.session_db.clone(),
                };
                cmd.execute().await
            }
        }
    }
}
