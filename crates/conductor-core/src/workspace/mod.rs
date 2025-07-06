use async_trait::async_trait;
use conductor_tools::{ToolCall, ToolSchema, result::ToolResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::EnvironmentInfo;
use crate::error::{Result, WorkspaceError};
use crate::session::state::WorkspaceConfig;
use crate::tools::ExecutionContext;

pub mod local;
pub mod remote;

/// Core workspace abstraction that owns both environment information and tool execution
#[async_trait]
pub trait Workspace: Send + Sync {
    /// Get environment information for this workspace
    async fn environment(&self) -> Result<EnvironmentInfo>;

    /// Execute a tool in this workspace
    async fn execute_tool(&self, tool_call: &ToolCall, ctx: ExecutionContext)
    -> Result<ToolResult>;

    /// Get available tools for this workspace
    async fn available_tools(&self) -> Vec<ToolSchema>;

    /// Check if a tool requires approval in this workspace
    async fn requires_approval(&self, tool_name: &str) -> Result<bool>;

    /// Get workspace metadata
    fn metadata(&self) -> WorkspaceMetadata;

    /// Invalidate cached environment information (force refresh on next call)
    async fn invalidate_environment_cache(&self);

    /// List files in the workspace for fuzzy finding
    /// Returns workspace-relative paths, filtered by optional query
    async fn list_files(
        &self,
        query: Option<&str>,
        max_results: Option<usize>,
    ) -> Result<Vec<String>>;
}

/// Metadata about a workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMetadata {
    pub id: String,
    pub workspace_type: WorkspaceType,
    pub location: String, // local path, remote URL, or container ID
}

/// Type of workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceType {
    Local,
    Remote,
}

impl WorkspaceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkspaceType::Local => "Local",
            WorkspaceType::Remote => "Remote",
        }
    }
}

/// Cached environment information with TTL
#[derive(Debug, Clone)]
pub(crate) struct CachedEnvironment {
    pub info: EnvironmentInfo,
    pub cached_at: Instant,
    pub ttl: Duration,
}

impl CachedEnvironment {
    pub fn new(info: EnvironmentInfo, ttl: Duration) -> Self {
        Self {
            info,
            cached_at: Instant::now(),
            ttl,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.cached_at.elapsed() > self.ttl
    }
}

/// Create a workspace from configuration
pub async fn create_workspace(config: &WorkspaceConfig) -> Result<Arc<dyn Workspace>> {
    match config {
        WorkspaceConfig::Local { path } => {
            let workspace = local::LocalWorkspace::with_path(path.clone()).await?;
            Ok(Arc::new(workspace))
        }
        WorkspaceConfig::Remote { .. } => {
            // Remote workspaces are not supported in conductor-core
            // They must be created through conductor-grpc
            Err(WorkspaceError::NotSupported(
                "Remote workspaces require conductor-grpc. Use conductor-grpc to create remote workspaces.".to_string()
            ).into())
        }
        WorkspaceConfig::Container { .. } => {
            // For now, container workspaces are not supported
            Err(WorkspaceError::NotSupported(
                "Container workspaces are not yet supported.".to_string(),
            )
            .into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::state::WorkspaceConfig;

    #[tokio::test]
    async fn test_create_local_workspace() {
        let config = WorkspaceConfig::Local {
            path: std::path::PathBuf::from("/test/path"),
        };
        let workspace = create_workspace(&config).await.unwrap();

        let metadata = workspace.metadata();
        assert!(matches!(metadata.workspace_type, WorkspaceType::Local));
    }

    #[tokio::test]
    async fn test_cached_environment_expiry() {
        let env_info = EnvironmentInfo {
            working_directory: std::env::current_dir().unwrap(),
            is_git_repo: false,
            platform: "test".to_string(),
            date: "2025-01-01".to_string(),
            directory_structure: "test/".to_string(),
            git_status: None,
            readme_content: None,
            claude_md_content: None,
        };

        let cached = CachedEnvironment::new(env_info, Duration::from_millis(1));
        assert!(!cached.is_expired());

        tokio::time::sleep(Duration::from_millis(2)).await;
        assert!(cached.is_expired());
    }
}
