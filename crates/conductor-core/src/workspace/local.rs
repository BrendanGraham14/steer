use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use conductor_tools::{ToolCall, ToolSchema};

use super::{CachedEnvironment, Workspace, WorkspaceMetadata, WorkspaceType};
use crate::app::EnvironmentInfo;
use crate::tools::{ExecutionContext, LocalBackend, ToolBackend};

/// Local filesystem workspace
pub struct LocalWorkspace {
    root_path: PathBuf,
    environment_cache: Arc<RwLock<Option<CachedEnvironment>>>,
    tool_backend: Arc<LocalBackend>,
    metadata: WorkspaceMetadata,
}

impl LocalWorkspace {
    pub async fn new() -> Result<Self> {
        Self::with_path(std::env::current_dir()?).await
    }

    pub async fn with_path(root_path: PathBuf) -> Result<Self> {
        let tool_backend = Arc::new(LocalBackend::workspace_only());
        let metadata = WorkspaceMetadata {
            id: format!("local:{}", root_path.display()),
            workspace_type: WorkspaceType::Local,
            location: root_path.display().to_string(),
        };

        Ok(Self {
            root_path,
            environment_cache: Arc::new(RwLock::new(None)),
            tool_backend,
            metadata,
        })
    }

    /// Collect environment information for the local workspace
    async fn collect_environment(&self) -> Result<EnvironmentInfo> {
        EnvironmentInfo::collect_for_path(&self.root_path)
    }
}

#[async_trait]
impl Workspace for LocalWorkspace {
    async fn environment(&self) -> Result<EnvironmentInfo> {
        let mut cache = self.environment_cache.write().await;

        // Check if we have valid cached data
        if let Some(cached) = cache.as_ref() {
            if !cached.is_expired() {
                return Ok(cached.info.clone());
            }
        }

        // Collect fresh environment info
        let env_info = self.collect_environment().await?;

        // Cache it with 5 minute TTL
        *cache = Some(CachedEnvironment::new(
            env_info.clone(),
            Duration::from_secs(300), // 5 minutes
        ));

        Ok(env_info)
    }

    async fn execute_tool(
        &self,
        tool_call: &ToolCall,
        mut ctx: ExecutionContext,
    ) -> Result<String> {
        // Set the working directory for local execution
        ctx.environment = crate::tools::ExecutionEnvironment::Local {
            working_directory: self.root_path.clone(),
        };

        self.tool_backend
            .execute(tool_call, &ctx)
            .await
            .map_err(|e| anyhow::anyhow!("Tool execution failed: {}", e))
    }

    async fn available_tools(&self) -> Vec<ToolSchema> {
        self.tool_backend.get_tool_schemas().await
    }

    async fn requires_approval(&self, tool_name: &str) -> Result<bool> {
        self.tool_backend
            .requires_approval(tool_name)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to check tool approval: {}", e))
    }

    fn metadata(&self) -> WorkspaceMetadata {
        self.metadata.clone()
    }

    async fn invalidate_environment_cache(&self) {
        let mut cache = self.environment_cache.write().await;
        *cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_local_workspace_creation() {
        let workspace = LocalWorkspace::new().await.unwrap();
        let metadata = workspace.metadata();

        assert!(matches!(metadata.workspace_type, WorkspaceType::Local));
        assert!(!metadata.location.is_empty());
    }

    #[tokio::test]
    async fn test_local_workspace_environment_caching() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // First call should populate cache
        let env1 = workspace.environment().await.unwrap();

        // Second call should use cache (should be fast)
        let env2 = workspace.environment().await.unwrap();

        assert_eq!(env1.working_directory, env2.working_directory);
    }

    #[tokio::test]
    async fn test_local_workspace_cache_invalidation() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // Populate cache
        let _env1 = workspace.environment().await.unwrap();

        // Invalidate cache
        workspace.invalidate_environment_cache().await;

        // Next call should re-collect
        let _env2 = workspace.environment().await.unwrap();

        // Should succeed without errors
    }

    #[tokio::test]
    async fn test_local_workspace_with_git_repo() {
        let temp_dir = tempdir().unwrap();

        // Create a git repo
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        let env = match workspace.environment().await {
            Ok(e) => e,
            Err(e) => {
                panic!("Environment collection failed: {}", e);
            }
        };

        // Should detect as git repo if git is available
        let expected_path = temp_dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());

        // Canonicalize both paths for comparison on macOS
        let actual_canonical = env
            .working_directory
            .canonicalize()
            .unwrap_or(env.working_directory.clone());
        let expected_canonical = expected_path
            .canonicalize()
            .unwrap_or(expected_path.clone());

        assert_eq!(actual_canonical, expected_canonical);
    }
}
