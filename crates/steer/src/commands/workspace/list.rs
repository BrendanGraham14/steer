use async_trait::async_trait;
use eyre::Result;

use super::super::Command;
use super::connect_client;
use steer_core::workspace::VcsKind;

pub struct ListWorkspaceCommand {
    pub environment_id: Option<String>,
    pub remote: Option<String>,
}

#[async_trait]
impl Command for ListWorkspaceCommand {
    async fn execute(&self) -> Result<()> {
        let client = connect_client(self.remote.as_deref()).await?;
        let workspaces = client
            .list_workspaces(self.environment_id.clone())
            .await?;

        if workspaces.is_empty() {
            println!("No workspaces found.");
            return Ok(());
        }

        println!("{:<36} {:<16} {:<6} {}", "ID", "Name", "VCS", "Path");
        println!("{}", "-".repeat(96));

        for workspace in workspaces {
            let name = workspace.name.unwrap_or_else(|| "-".to_string());
            let vcs = workspace
                .vcs_kind
                .as_ref()
                .map(VcsKind::as_str)
                .unwrap_or("-");
            println!(
                "{:<36} {:<16} {:<6} {}",
                workspace.workspace_id.as_uuid(),
                name,
                vcs,
                workspace.path.display()
            );
        }

        Ok(())
    }
}
