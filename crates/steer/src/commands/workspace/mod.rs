use async_trait::async_trait;
use eyre::{Result, eyre};

use super::Command;
use crate::cli::WorkspaceCommands;

mod list;
mod status;

pub use list::ListWorkspaceCommand;
pub use status::WorkspaceStatusCommand;

pub struct WorkspaceCommand {
    pub command: WorkspaceCommands,
    pub remote: Option<String>,
    pub session_id: Option<String>,
}

#[async_trait]
impl Command for WorkspaceCommand {
    async fn execute(&self) -> Result<()> {
        match &self.command {
            WorkspaceCommands::List {
                environment_id,
            } => {
                let cmd = ListWorkspaceCommand {
                    environment_id: environment_id.clone(),
                    remote: self.remote.clone(),
                };
                cmd.execute().await
            }
            WorkspaceCommands::Status {
                workspace_id,
                session_id,
            } => {
                let cmd = WorkspaceStatusCommand {
                    workspace_id: workspace_id.clone(),
                    session_id: session_id.clone().or_else(|| self.session_id.clone()),
                    remote: self.remote.clone(),
                };
                cmd.execute().await
            }
        }
    }
}

async fn connect_client(remote: Option<&str>) -> Result<steer_grpc::AgentClient> {
    if let Some(addr) = remote {
        return steer_grpc::AgentClient::connect(addr)
            .await
            .map_err(|e| eyre!("Failed to connect to remote server at {addr}: {e}"));
    }

    let default_model = steer_core::config::model::builtin::claude_sonnet_4_5();
    steer_grpc::AgentClient::local(default_model)
        .await
        .map_err(|e| eyre!("Failed to start local gRPC server: {e}"))
}
