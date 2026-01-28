use async_trait::async_trait;
use eyre::Result;

use super::super::Command;
use super::connect_client;

pub struct ListWorkspaceCommand {
    pub environment_id: Option<String>,
    pub remote: Option<String>,
}

#[async_trait]
impl Command for ListWorkspaceCommand {
    async fn execute(&self) -> Result<()> {
        let client = connect_client(self.remote.as_deref()).await?;
        let workspaces = client.list_workspaces(self.environment_id.clone()).await?;

        if workspaces.is_empty() {
            println!("No workspaces found.");
            return Ok(());
        }

        println!("{:<36} {:<16} {:<36} Path", "Workspace", "Name", "Repo");
        println!("{}", "-".repeat(128));

        for workspace in workspaces {
            let name = workspace.name.unwrap_or_else(|| "-".to_string());
            println!(
                "{:<36} {:<16} {:<36} {}",
                workspace.workspace_id.as_uuid(),
                name,
                workspace.repo_id.as_uuid(),
                workspace.path.display()
            );
        }

        Ok(())
    }
}
