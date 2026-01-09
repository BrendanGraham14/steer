use async_trait::async_trait;
use steer_tools::{ToolCall, ToolSchema, result::ToolResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::Result;
use steer_tools::ExecutionContext;

pub mod local;
pub mod remote;
pub mod config;

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

/// Environment information for a workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub working_directory: std::path::PathBuf,
    pub is_git_repo: bool,
    pub platform: String,
    pub date: String,
    pub directory_structure: String,
    pub git_status: Option<String>,
    pub readme_content: Option<String>,
    pub memory_file_name: Option<String>,
    pub memory_file_content: Option<String>,
}

impl EnvironmentInfo {
    /// Collect environment information for a given path
    pub fn collect_for_path(path: &std::path::Path) -> Result<Self> {
        use crate::utils::{DirectoryStructureUtils, EnvironmentUtils, GitStatusUtils};

        let is_git_repo = EnvironmentUtils::is_git_repo(path);
        let platform = EnvironmentUtils::get_platform().to_string();
        let date = EnvironmentUtils::get_current_date();
        
        let directory_structure = DirectoryStructureUtils::get_directory_structure(path, 3)?;
        
        let git_status = if is_git_repo {
            GitStatusUtils::get_git_status(path).ok()
        } else {
            None
        };

        let readme_content = EnvironmentUtils::read_readme(path);
        let (memory_file_name, memory_file_content) =
            match EnvironmentUtils::read_memory_file(path) {
                Some((name, content)) => (Some(name), Some(content)),
                None => (None, None),
            };

        Ok(Self {
            working_directory: path.to_path_buf(),
            is_git_repo,
            platform,
            date,
            directory_structure,
            git_status,
            readme_content,
            memory_file_name,
            memory_file_content,
        })
    }
}

/// Configuration for a workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceConfig {
    /// Local filesystem workspace
    Local {
        /// Path to the workspace directory
        path: std::path::PathBuf,
    },
    /// Remote workspace accessed via gRPC
    Remote {
        /// Address of the remote workspace service (e.g., "localhost:50051")
        address: String,
        /// Optional authentication for the remote service
        auth: Option<RemoteAuth>,
    },
}

/// Authentication information for remote workspaces
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RemoteAuth {
    /// Bearer token authentication
    BearerToken(String),
    /// API key authentication
    ApiKey(String),
}

/// Create a workspace from configuration
pub async fn create_workspace(config: &WorkspaceConfig) -> Result<Arc<dyn Workspace>> {
    match config {
        WorkspaceConfig::Local { path } => {
            let workspace = local::LocalWorkspace::with_path(path.clone()).await?;
            Ok(Arc::new(workspace))
        }
        WorkspaceConfig::Remote { address, auth } => {
            let workspace = remote::RemoteWorkspace::new(address.clone(), auth.clone()).await?;
            Ok(Arc::new(workspace))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            memory_file_name: None,
            memory_file_content: None,
        };

        let cached = CachedEnvironment::new(env_info, Duration::from_millis(1));
        assert!(!cached.is_expired());

        tokio::time::sleep(Duration::from_millis(2)).await;
        assert!(cached.is_expired());
    }
}
