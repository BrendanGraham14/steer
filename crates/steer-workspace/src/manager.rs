use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::{EnvironmentManagerResult, WorkspaceManagerResult};
use crate::{
    EnvironmentId, EnvironmentInfo, Workspace, WorkspaceId, WorkspaceInfo, WorkspaceRef,
    WorkspaceStatus,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnvironmentRequest {
    pub root: Option<std::path::PathBuf>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EnvironmentDeletePolicy {
    Hard,
    Soft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentDescriptor {
    pub environment_id: EnvironmentId,
    pub root: std::path::PathBuf,
}

#[async_trait]
pub trait EnvironmentManager: Send + Sync + std::fmt::Debug {
    async fn create_environment(
        &self,
        request: CreateEnvironmentRequest,
    ) -> EnvironmentManagerResult<EnvironmentDescriptor>;

    async fn get_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> EnvironmentManagerResult<EnvironmentDescriptor>;

    async fn delete_environment(
        &self,
        environment_id: EnvironmentId,
        policy: EnvironmentDeletePolicy,
    ) -> EnvironmentManagerResult<()>;

    async fn environment_info(
        &self,
        environment_id: EnvironmentId,
    ) -> EnvironmentManagerResult<EnvironmentInfo>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceCreateStrategy {
    JjWorkspace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub base: Option<WorkspaceRef>,
    pub name: Option<String>,
    pub parent_workspace_id: Option<WorkspaceId>,
    pub strategy: WorkspaceCreateStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListWorkspacesRequest {
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteWorkspaceRequest {
    pub workspace_id: WorkspaceId,
}

#[async_trait]
pub trait WorkspaceManager: Send + Sync + std::fmt::Debug {
    async fn create_workspace(
        &self,
        request: CreateWorkspaceRequest,
    ) -> WorkspaceManagerResult<WorkspaceInfo>;

    async fn list_workspaces(
        &self,
        request: ListWorkspacesRequest,
    ) -> WorkspaceManagerResult<Vec<WorkspaceInfo>>;

    async fn open_workspace(
        &self,
        workspace_id: WorkspaceId,
    ) -> WorkspaceManagerResult<Arc<dyn Workspace>>;

    async fn get_workspace_status(
        &self,
        workspace_id: WorkspaceId,
    ) -> WorkspaceManagerResult<WorkspaceStatus>;

    async fn delete_workspace(&self, request: DeleteWorkspaceRequest)
    -> WorkspaceManagerResult<()>;
}
