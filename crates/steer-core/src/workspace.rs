use steer_workspace::WorkspaceError;
pub use steer_workspace::{
    CreateWorkspaceRequest, EnvironmentId, EnvironmentInfo, ListWorkspacesRequest,
    LocalWorkspaceManager, RemoteAuth, Workspace, WorkspaceConfig, WorkspaceCreateStrategy,
    WorkspaceId, WorkspaceInfo, WorkspaceManager, WorkspaceMetadata, WorkspaceRef,
    WorkspaceStatus, WorkspaceType, LlmStatus, VcsInfo, VcsKind, VcsStatus,
};

use crate::error::Result;
use std::sync::Arc;

/// Create a workspace from configuration
pub async fn create_workspace(
    config: &steer_workspace::WorkspaceConfig,
) -> Result<Arc<dyn Workspace>> {
    match config {
        steer_workspace::WorkspaceConfig::Local { path } => {
            let workspace = steer_workspace::local::LocalWorkspace::with_path(path.clone())
                .await
                .map_err(|e| WorkspaceError::NotSupported(e.to_string()))?;
            Ok(Arc::new(workspace))
        }
        steer_workspace::WorkspaceConfig::Remote { address, auth } => {
            let workspace =
                steer_workspace_client::RemoteWorkspace::new(address.clone(), auth.clone())
                    .await
                    .map_err(|e| WorkspaceError::NotSupported(e.to_string()))?;
            Ok(Arc::new(workspace))
        }
    }
}

/// Create a workspace from the current session configuration
/// This is a compatibility wrapper for steer-core usage
pub async fn create_workspace_from_session_config(
    config: &crate::session::state::WorkspaceConfig,
) -> Result<Arc<dyn Workspace>> {
    use steer_workspace::WorkspaceConfig as WsConfig;

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
                    steer_workspace::RemoteAuth::BearerToken(token.clone())
                }
                crate::session::state::RemoteAuth::ApiKey { key } => {
                    steer_workspace::RemoteAuth::ApiKey(key.clone())
                }
            });
            WsConfig::Remote {
                address: agent_address.clone(),
                auth: ws_auth,
            }
        }
    };

    create_workspace(&ws_config)
        .await
        .map_err(|e| WorkspaceError::NotSupported(e.to_string()).into())
}
