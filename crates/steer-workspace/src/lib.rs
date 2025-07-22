pub mod config;
pub mod error;
pub mod local;
pub mod utils;

// Re-export main types
pub use config::{RemoteAuth, WorkspaceConfig};
pub use error::{Result, WorkspaceError};

// Module with the trait and core types
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Core workspace abstraction for environment information and file operations
#[async_trait]
pub trait Workspace: Send + Sync + std::fmt::Debug {
    /// Get environment information for this workspace
    async fn environment(&self) -> Result<EnvironmentInfo>;

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

    /// Get the working directory for this workspace
    fn working_directory(&self) -> &std::path::Path;

    /// Execute a tool in this workspace
    async fn execute_tool(
        &self,
        tool_call: &steer_tools::ToolCall,
        context: steer_tools::ExecutionContext,
    ) -> Result<steer_tools::result::ToolResult>;

    /// Get available tools in this workspace
    async fn available_tools(&self) -> Vec<steer_tools::ToolSchema>;

    /// Check if a tool requires approval
    async fn requires_approval(&self, tool_name: &str) -> Result<bool>;
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
    pub claude_md_content: Option<String>,
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
        let claude_md_content = EnvironmentUtils::read_claude_md(path);

        Ok(Self {
            working_directory: path.to_path_buf(),
            is_git_repo,
            platform,
            date,
            directory_structure,
            git_status,
            readme_content,
            claude_md_content,
        })
    }

    /// Format environment info as context for system prompt
    pub fn as_context(&self) -> String {
        let mut context = format!(
            "Here is useful information about the environment you are running in:\n<env>\nWorking directory: {}\nIs directory a git repo: {}\nPlatform: {}\nToday's date: {}\n</env>",
            self.working_directory.display(),
            self.is_git_repo,
            self.platform,
            self.date
        );

        if !self.directory_structure.is_empty() {
            context.push_str(&format!("\n\n<file_structure>\nBelow is a snapshot of this project's file structure at the start of the conversation. The file structure may be filtered to omit `.gitignore`ed patterns. This snapshot will NOT update during the conversation.\n\n{}\n</file_structure>", self.directory_structure));
        }

        if let Some(ref git_status) = self.git_status {
            context.push_str(&format!("\n<git_status>\nThis is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.\n\n{git_status}\n</git_status>"));
        }

        if let Some(ref readme) = self.readme_content {
            context.push_str(&format!("\n<file name=\"README.md\">\nThis is the README.md file at the start of the conversation. Note that this README is a snapshot in time, and will not update during the conversation.\n\n{readme}\n</file>"));
        }

        if let Some(ref claude_md) = self.claude_md_content {
            context.push_str(&format!("\n<file name=\"CLAUDE.md\">\nThis is the CLAUDE.md file at the start of the conversation. Note that this CLAUDE is a snapshot in time, and will not update during the conversation.\n\n{claude_md}\n</file>"));
        }

        context
    }
}
