// Re-export workspace types from conductor-workspace crate
pub use conductor_workspace::{
    EnvironmentInfo, RemoteAuth, Workspace, WorkspaceConfig, WorkspaceMetadata, WorkspaceType,
};

// Create type aliases for backwards compatibility with session state
use crate::error::{Result, WorkspaceError};
use std::sync::Arc;

/// Create a workspace from configuration
pub async fn create_workspace(
    config: &conductor_workspace::WorkspaceConfig,
) -> Result<Arc<dyn Workspace>> {
    match config {
        conductor_workspace::WorkspaceConfig::Local { path } => {
            let workspace = conductor_workspace::local::LocalWorkspace::with_path(path.clone())
                .await
                .map_err(|e| WorkspaceError::NotSupported(e.to_string()))?;
            Ok(Arc::new(workspace))
        }
        conductor_workspace::WorkspaceConfig::Remote { address, auth } => {
            let workspace =
                conductor_workspace_client::RemoteWorkspace::new(address.clone(), auth.clone())
                    .await
                    .map_err(|e| WorkspaceError::NotSupported(e.to_string()))?;
            Ok(Arc::new(workspace))
        }
        conductor_workspace::WorkspaceConfig::Container { .. } => Err(
            WorkspaceError::NotSupported("Container workspaces are not yet supported.".to_string())
                .into(),
        ),
    }
}

/// Create a workspace from the current session configuration
/// This is a compatibility wrapper for conductor-core usage
pub async fn create_workspace_from_session_config(
    config: &crate::session::state::WorkspaceConfig,
) -> Result<Arc<dyn Workspace>> {
    use conductor_workspace::WorkspaceConfig as WsConfig;

    let ws_config = match config {
        crate::session::state::WorkspaceConfig::Local { path } => {
            WsConfig::Local { path: path.clone() }
        }
        crate::session::state::WorkspaceConfig::Remote {
            agent_address,
            auth,
        } => {
            let ws_auth = auth.as_ref().map(|a| match a {
                crate::session::state::RemoteAuth::Bearer { token } => {
                    conductor_workspace::RemoteAuth::BearerToken(token.clone())
                }
                crate::session::state::RemoteAuth::ApiKey { key } => {
                    conductor_workspace::RemoteAuth::ApiKey(key.clone())
                }
            });
            WsConfig::Remote {
                address: agent_address.clone(),
                auth: ws_auth,
            }
        }
        crate::session::state::WorkspaceConfig::Container { .. } => {
            return Err(WorkspaceError::NotSupported(
                "Container workspaces are not yet supported.".to_string(),
            )
            .into());
        }
    };

    create_workspace(&ws_config)
        .await
        .map_err(|e| WorkspaceError::NotSupported(e.to_string()).into())
}
