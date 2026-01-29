use async_trait::async_trait;
use eyre::{Result, eyre};
use std::io::Write;

use super::super::Command;
use super::connect_client;
use steer_core::workspace::{LlmStatus, VcsKind};

pub struct WorkspaceStatusCommand {
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    pub remote: Option<String>,
}

#[async_trait]
impl Command for WorkspaceStatusCommand {
    async fn execute(&self) -> Result<()> {
        let client = connect_client(self.remote.as_deref()).await?;
        let workspace_id = self.resolve_workspace_id(&client).await?;
        let status = client.get_workspace_status(&workspace_id).await?;

        let mut stdout = std::io::stdout();
        writeln!(stdout, "{}", format_workspace_status(&status))?;
        Ok(())
    }
}

impl WorkspaceStatusCommand {
    async fn resolve_workspace_id(&self, client: &steer_grpc::AgentClient) -> Result<String> {
        if let Some(workspace_id) = &self.workspace_id {
            return Ok(workspace_id.clone());
        }

        let session_id = self
            .session_id
            .as_ref()
            .ok_or_else(|| eyre!("Provide --workspace-id or --session-id"))?;

        let session = client
            .get_session(session_id)
            .await?
            .ok_or_else(|| eyre!("Session not found: {session_id}"))?;

        let config = session
            .config
            .ok_or_else(|| eyre!("Session config missing for {session_id}"))?;

        if let Some(workspace_id) = config.workspace_id {
            return Ok(workspace_id);
        }

        if let Some(reference) = config.workspace_ref {
            return Ok(reference.workspace_id);
        }

        Err(eyre!("Session has no workspace id"))
    }
}

fn format_workspace_status(status: &steer_core::workspace::WorkspaceStatus) -> String {
    let mut output = String::new();
    output.push_str(&format!("Workspace: {}\n", status.workspace_id.as_uuid()));
    output.push_str(&format!(
        "Environment: {}\n",
        status.environment_id.as_uuid()
    ));
    output.push_str(&format!("Repo: {}\n", status.repo_id.as_uuid()));
    output.push_str(&format!("Path: {}\n", status.path.display()));

    match &status.vcs {
        Some(vcs) => {
            let kind = match vcs.kind {
                VcsKind::Git => "git",
                VcsKind::Jj => "jj",
            };
            output.push_str(&format!("VCS: {} ({})\n\n", kind, vcs.root.display()));
            output.push_str(&vcs.status.as_llm_string());
        }
        None => {
            output.push_str("VCS: <none>\n");
        }
    }

    output
}
