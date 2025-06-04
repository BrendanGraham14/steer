use anyhow::Result;
use async_trait::async_trait;

use super::Command;
use crate::cli::SessionCommands;

mod create;
mod delete;
mod list;
mod resume;
mod show;

pub use create::CreateSessionCommand;
pub use delete::DeleteSessionCommand;
pub use list::ListSessionCommand;
pub use resume::{LatestSessionCommand, ResumeSessionCommand};
pub use show::ShowSessionCommand;

pub struct SessionCommand {
    pub command: SessionCommands,
    pub remote: Option<String>,
}

#[async_trait]
impl Command for SessionCommand {
    async fn execute(&self) -> Result<()> {
        // Dispatch to appropriate subcommand
        match &self.command {
            SessionCommands::List { active, limit } => {
                let cmd = ListSessionCommand {
                    active: *active,
                    limit: *limit,
                    remote: self.remote.clone(),
                };
                cmd.execute().await
            }
            SessionCommands::Create {
                tool_policy,
                pre_approved_tools,
                metadata,
            } => {
                let cmd = CreateSessionCommand {
                    tool_policy: tool_policy.clone(),
                    pre_approved_tools: pre_approved_tools.clone(),
                    metadata: metadata.clone(),
                    remote: self.remote.clone(),
                };
                cmd.execute().await
            }
            SessionCommands::Resume { session_id } => {
                let cmd = ResumeSessionCommand {
                    session_id: session_id.clone(),
                    remote: self.remote.clone(),
                };
                cmd.execute().await
            }
            SessionCommands::Latest => {
                let cmd = LatestSessionCommand {
                    remote: self.remote.clone(),
                };
                cmd.execute().await
            }
            SessionCommands::Delete { session_id, force } => {
                let cmd = DeleteSessionCommand {
                    session_id: session_id.clone(),
                    force: *force,
                    remote: self.remote.clone(),
                };
                cmd.execute().await
            }
            SessionCommands::Show { session_id } => {
                let cmd = ShowSessionCommand {
                    session_id: session_id.clone(),
                    remote: self.remote.clone(),
                };
                cmd.execute().await
            }
        }
    }
}
