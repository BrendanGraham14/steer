use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::info;

use crate::error::{Result, WorkspaceError};
use crate::{CachedEnvironment, EnvironmentInfo, Workspace, WorkspaceMetadata, WorkspaceType};
use steer_tools::{
    ExecutionContext as SteerExecutionContext, ToolCall, ToolSchema, result::ToolResult,
    traits::ExecutableTool,
};

/// Local filesystem workspace
pub struct LocalWorkspace {
    path: PathBuf,
    environment_cache: Arc<RwLock<Option<CachedEnvironment>>>,
    metadata: WorkspaceMetadata,
    tool_registry: HashMap<String, Box<dyn ExecutableTool>>,
}

impl LocalWorkspace {
    pub async fn with_path(path: PathBuf) -> Result<Self> {
        let metadata = WorkspaceMetadata {
            id: format!("local:{}", path.display()),
            workspace_type: WorkspaceType::Local,
            location: path.display().to_string(),
        };

        // Create tool registry from workspace tools
        let mut tool_registry = HashMap::new();
        for tool in steer_tools::tools::workspace_tools() {
            tool_registry.insert(tool.name().to_string(), tool);
        }

        Ok(Self {
            path,
            environment_cache: Arc::new(RwLock::new(None)),
            metadata,
            tool_registry,
        })
    }

    /// Collect environment information for the local workspace
    async fn collect_environment(&self) -> Result<EnvironmentInfo> {
        EnvironmentInfo::collect_for_path(&self.path)
    }
}

impl std::fmt::Debug for LocalWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalWorkspace")
            .field("path", &self.path)
            .field("metadata", &self.metadata)
            .field("tool_count", &self.tool_registry.len())
            .finish_non_exhaustive()
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

    fn metadata(&self) -> WorkspaceMetadata {
        self.metadata.clone()
    }

    async fn invalidate_environment_cache(&self) {
        let mut cache = self.environment_cache.write().await;
        *cache = None;
    }

    async fn list_files(
        &self,
        query: Option<&str>,
        max_results: Option<usize>,
    ) -> Result<Vec<String>> {
        use crate::utils::FileListingUtils;

        info!(target: "workspace.list_files", "Listing files in workspace: {:?}", self.path);

        FileListingUtils::list_files(&self.path, query, max_results).map_err(WorkspaceError::from)
    }

    fn working_directory(&self) -> &std::path::Path {
        &self.path
    }

    async fn execute_tool(
        &self,
        tool_call: &ToolCall,
        context: steer_tools::ExecutionContext,
    ) -> Result<ToolResult> {
        // Get the tool from registry
        let tool = self.tool_registry.get(&tool_call.name).ok_or_else(|| {
            WorkspaceError::ToolExecution(format!("Unknown tool: {}", tool_call.name))
        })?;

        // Set working directory
        let steer_context = SteerExecutionContext::new(tool_call.id.clone())
            .with_cancellation_token(context.cancellation_token.clone())
            .with_working_directory(self.path.clone());

        // Execute the tool
        tool.run(tool_call.parameters.clone(), &steer_context)
            .await
            .map_err(|e| WorkspaceError::ToolExecution(e.to_string()))
    }

    async fn available_tools(&self) -> Vec<ToolSchema> {
        self.tool_registry
            .iter()
            .map(|(name, tool)| ToolSchema {
                name: name.clone(),
                description: tool.description().to_string(),
                input_schema: tool.input_schema().clone(),
            })
            .collect()
    }

    async fn requires_approval(&self, tool_name: &str) -> Result<bool> {
        self.tool_registry
            .get(tool_name)
            .map(|tool| tool.requires_approval())
            .ok_or_else(|| WorkspaceError::ToolExecution(format!("Unknown tool: {tool_name}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_local_workspace_creation() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();
        assert!(matches!(
            workspace.metadata().workspace_type,
            WorkspaceType::Local
        ));
    }

    #[tokio::test]
    async fn test_local_workspace_with_path() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        assert!(matches!(
            workspace.metadata().workspace_type,
            WorkspaceType::Local
        ));
        assert_eq!(
            workspace.metadata().location,
            temp_dir.path().display().to_string()
        );
    }

    #[tokio::test]
    async fn test_environment_caching() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // First call should collect fresh data
        let env1 = workspace.environment().await.unwrap();

        // Second call should return cached data
        let env2 = workspace.environment().await.unwrap();

        // Should be identical
        assert_eq!(env1.working_directory, env2.working_directory);
        assert_eq!(env1.is_git_repo, env2.is_git_repo);
        assert_eq!(env1.platform, env2.platform);
    }

    #[tokio::test]
    async fn test_cache_invalidation() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // Get initial environment
        let _ = workspace.environment().await.unwrap();

        // Invalidate cache
        workspace.invalidate_environment_cache().await;

        // Should work fine and fetch fresh data
        let env = workspace.environment().await.unwrap();
        assert!(!env.working_directory.as_os_str().is_empty());
    }

    #[tokio::test]
    async fn test_environment_collection() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        let env = workspace.environment().await.unwrap();

        // Verify basic environment info
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

    #[tokio::test]
    async fn test_list_files() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // Create some test files
        std::fs::write(temp_dir.path().join("test.rs"), "test").unwrap();
        std::fs::write(temp_dir.path().join("main.rs"), "main").unwrap();
        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        std::fs::write(temp_dir.path().join("src/lib.rs"), "lib").unwrap();

        // List all files
        let files = workspace.list_files(None, None).await.unwrap();
        assert_eq!(files.len(), 4); // 3 files + 1 directory
        assert!(files.contains(&"test.rs".to_string()));
        assert!(files.contains(&"main.rs".to_string()));
        assert!(files.contains(&"src/".to_string())); // Directory with trailing slash
        assert!(files.contains(&"src/lib.rs".to_string()));

        // Test with query
        let files = workspace.list_files(Some("test"), None).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], "test.rs");

        // Test with max_results
        let files = workspace.list_files(None, Some(2)).await.unwrap();
        assert_eq!(files.len(), 2);
    }

    #[tokio::test]
    async fn test_list_files_includes_dotfiles() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // Create a dotfile
        std::fs::write(temp_dir.path().join(".gitignore"), "target/").unwrap();

        let files = workspace.list_files(None, None).await.unwrap();
        assert!(files.contains(&".gitignore".to_string()));
    }

    #[tokio::test]
    async fn test_working_directory() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        assert_eq!(workspace.working_directory(), temp_dir.path());
    }
}
