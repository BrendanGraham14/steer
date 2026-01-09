pub mod config;
pub mod error;
pub mod local;
pub mod local_manager;
pub mod manager;
pub mod ops;
pub mod result;
pub mod utils;
mod workspace_registry;

// Re-export main types
pub use config::{RemoteAuth, WorkspaceConfig};
pub use error::{
    EnvironmentManagerError, EnvironmentManagerResult, Result, WorkspaceError,
    WorkspaceManagerError, WorkspaceManagerResult,
};
pub use local::LocalEnvironmentManager;
pub use local_manager::LocalWorkspaceManager;
pub use manager::{
    CreateEnvironmentRequest, CreateWorkspaceRequest, DeleteWorkspaceRequest, EnvironmentDeletePolicy,
    EnvironmentDescriptor, EnvironmentManager, ListWorkspacesRequest, RepoManager,
    WorkspaceCreateStrategy, WorkspaceManager,
};
pub use ops::{
    ApplyEditsRequest, AstGrepRequest, EditOperation, GlobRequest, GrepRequest,
    ListDirectoryRequest, ReadFileRequest, WorkspaceOpContext, WriteFileRequest,
};
pub use result::{
    EditResult, FileContentResult, FileEntry, FileListResult, GlobResult, SearchMatch, SearchResult,
};

// Module with the trait and core types
use async_trait::async_trait;
#[cfg(feature = "schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tracing::debug;
use uuid::Uuid;

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

    /// Read file contents with optional offset/limit.
    async fn read_file(
        &self,
        request: ReadFileRequest,
        ctx: &WorkspaceOpContext,
    ) -> Result<FileContentResult>;

    /// List a directory (similar to ls).
    async fn list_directory(
        &self,
        request: ListDirectoryRequest,
        ctx: &WorkspaceOpContext,
    ) -> Result<FileListResult>;

    /// Apply glob patterns.
    async fn glob(&self, request: GlobRequest, ctx: &WorkspaceOpContext) -> Result<GlobResult>;

    /// Text search (grep-style).
    async fn grep(&self, request: GrepRequest, ctx: &WorkspaceOpContext) -> Result<SearchResult>;

    /// AST search (astgrep-style).
    async fn astgrep(
        &self,
        request: AstGrepRequest,
        ctx: &WorkspaceOpContext,
    ) -> Result<SearchResult>;

    /// Apply one or more edits to a file.
    async fn apply_edits(
        &self,
        request: ApplyEditsRequest,
        ctx: &WorkspaceOpContext,
    ) -> Result<EditResult>;

    /// Write/replace entire file content.
    async fn write_file(
        &self,
        request: WriteFileRequest,
        ctx: &WorkspaceOpContext,
    ) -> Result<EditResult>;
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

/// Stable identifier for an execution environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct EnvironmentId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

impl Default for EnvironmentId {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvironmentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Identifier for the implicit local environment.
    pub fn local() -> Self {
        Self(Uuid::nil())
    }

    pub fn is_local(&self) -> bool {
        self.0.is_nil()
    }
}

/// Stable identifier for a workspace within an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct WorkspaceId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

impl Default for WorkspaceId {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// Stable identifier for a repository within an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct RepoId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

impl Default for RepoId {
    fn default() -> Self {
        Self::new()
    }
}

impl RepoId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// Reference to a repository inside a specific environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct RepoRef {
    pub environment_id: EnvironmentId,
    pub repo_id: RepoId,
    pub root_path: std::path::PathBuf,
    pub vcs_kind: Option<VcsKind>,
}

/// Repository metadata for listing and workspace grouping.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct RepoInfo {
    pub repo_id: RepoId,
    pub environment_id: EnvironmentId,
    pub root_path: std::path::PathBuf,
    pub vcs_kind: Option<VcsKind>,
}

/// Reference to a workspace inside a specific environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct WorkspaceRef {
    pub environment_id: EnvironmentId,
    pub workspace_id: WorkspaceId,
    pub repo_id: RepoId,
}

/// Workspace metadata for listing and UI grouping.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct WorkspaceInfo {
    pub workspace_id: WorkspaceId,
    pub environment_id: EnvironmentId,
    pub repo_id: RepoId,
    pub parent_workspace_id: Option<WorkspaceId>,
    pub name: Option<String>,
    pub path: std::path::PathBuf,
}

/// Workspace status for orchestration and UI display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub workspace_id: WorkspaceId,
    pub environment_id: EnvironmentId,
    pub repo_id: RepoId,
    pub path: std::path::PathBuf,
    pub vcs: Option<VcsInfo>,
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

/// Supported version control systems
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub enum VcsKind {
    Git,
    Jj,
}

impl VcsKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            VcsKind::Git => "git",
            VcsKind::Jj => "jj",
        }
    }
}

/// Trait for status types that can render LLM-readable summaries.
pub trait LlmStatus {
    fn as_llm_string(&self) -> String;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GitHead {
    Branch(String),
    Detached,
    Unborn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GitStatusSummary {
    Added,
    Removed,
    Modified,
    TypeChange,
    Renamed,
    Copied,
    IntentToAdd,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusEntry {
    pub summary: GitStatusSummary,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitSummary {
    pub id: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    pub head: Option<GitHead>,
    pub entries: Vec<GitStatusEntry>,
    pub recent_commits: Vec<GitCommitSummary>,
    pub error: Option<String>,
}

impl GitStatus {
    pub fn new(
        head: GitHead,
        entries: Vec<GitStatusEntry>,
        recent_commits: Vec<GitCommitSummary>,
    ) -> Self {
        Self {
            head: Some(head),
            entries,
            recent_commits,
            error: None,
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            head: None,
            entries: Vec::new(),
            recent_commits: Vec::new(),
            error: Some(message.into()),
        }
    }
}

impl LlmStatus for GitStatus {
    fn as_llm_string(&self) -> String {
        if let Some(error) = &self.error {
            return format!("Status unavailable: {error}");
        }
        let head = match &self.head {
            Some(head) => head,
            None => return "Status unavailable: missing git head".to_string(),
        };

        let mut result = String::new();
        match head {
            GitHead::Branch(branch) => {
                result.push_str(&format!("Current branch: {branch}\n\n"));
            }
            GitHead::Detached => {
                result.push_str("Current branch: HEAD (detached)\n\n");
            }
            GitHead::Unborn => {
                result.push_str("Current branch: <unborn>\n\n");
            }
        }

        result.push_str("Status:\n");
        if self.entries.is_empty() {
            result.push_str("Working tree clean\n");
        } else {
            for entry in &self.entries {
                let (status_char, wt_char) = match entry.summary {
                    GitStatusSummary::Added => (' ', '?'),
                    GitStatusSummary::Removed => ('D', ' '),
                    GitStatusSummary::Modified => ('M', ' '),
                    GitStatusSummary::TypeChange => ('T', ' '),
                    GitStatusSummary::Renamed => ('R', ' '),
                    GitStatusSummary::Copied => ('C', ' '),
                    GitStatusSummary::IntentToAdd => ('A', ' '),
                    GitStatusSummary::Conflict => ('U', 'U'),
                };
                result.push_str(&format!("{status_char}{wt_char} {}\n", entry.path));
            }
        }

        result.push_str("\nRecent commits:\n");
        if self.recent_commits.is_empty() {
            result.push_str("<no commits>\n");
        } else {
            for commit in &self.recent_commits {
                result.push_str(&format!("{} {}\n", commit.id, commit.summary));
            }
        }

        result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JjChangeType {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JjChange {
    pub change_type: JjChangeType,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JjCommitSummary {
    pub change_id: String,
    pub commit_id: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JjStatus {
    pub changes: Vec<JjChange>,
    pub working_copy: Option<JjCommitSummary>,
    pub parents: Vec<JjCommitSummary>,
    pub error: Option<String>,
}

impl JjStatus {
    pub fn new(
        changes: Vec<JjChange>,
        working_copy: JjCommitSummary,
        parents: Vec<JjCommitSummary>,
    ) -> Self {
        Self {
            changes,
            working_copy: Some(working_copy),
            parents,
            error: None,
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            changes: Vec::new(),
            working_copy: None,
            parents: Vec::new(),
            error: Some(message.into()),
        }
    }
}

impl LlmStatus for JjStatus {
    fn as_llm_string(&self) -> String {
        if let Some(error) = &self.error {
            return format!("Status unavailable: {error}");
        }
        let working_copy = match &self.working_copy {
            Some(working_copy) => working_copy,
            None => return "Status unavailable: missing jj working copy".to_string(),
        };

        let mut status = String::new();
        status.push_str("Working copy changes:\n");
        if self.changes.is_empty() {
            status.push_str("<none>\n");
        } else {
            for change in &self.changes {
                let status_char = match change.change_type {
                    JjChangeType::Added => 'A',
                    JjChangeType::Removed => 'D',
                    JjChangeType::Modified => 'M',
                };
                status.push_str(&format!("{status_char} {}\n", change.path));
            }
        }
        status.push_str(&format!(
            "Working copy (@): {} {} {}\n",
            working_copy.change_id, working_copy.commit_id, working_copy.description
        ));

        if self.parents.is_empty() {
            status.push_str("Parent commit (@-): <none>\n");
        } else {
            for (index, parent) in self.parents.iter().enumerate() {
                let marker = if index == 0 {
                    "(@-)".to_string()
                } else {
                    format!("(@-{})", index + 1)
                };
                status.push_str(&format!(
                    "Parent commit {marker}: {} {} {}\n",
                    parent.change_id, parent.commit_id, parent.description
                ));
            }
        }

        status
    }
}

/// VCS-specific status data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VcsStatus {
    Git(GitStatus),
    Jj(JjStatus),
}

impl LlmStatus for VcsStatus {
    fn as_llm_string(&self) -> String {
        match self {
            VcsStatus::Git(status) => status.as_llm_string(),
            VcsStatus::Jj(status) => status.as_llm_string(),
        }
    }
}

/// Version control information for a workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VcsInfo {
    pub kind: VcsKind,
    pub root: std::path::PathBuf,
    pub status: VcsStatus,
}

/// Environment information for a workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub working_directory: std::path::PathBuf,
    pub vcs: Option<VcsInfo>,
    pub platform: String,
    pub date: String,
    pub directory_structure: String,
    pub readme_content: Option<String>,
    pub memory_file_name: Option<String>,
    pub memory_file_content: Option<String>,
}

/// Default maximum depth for directory structure traversal
pub const MAX_DIRECTORY_DEPTH: usize = 3;

/// Default maximum number of items to include in directory structure
pub const MAX_DIRECTORY_ITEMS: usize = 1000;

impl EnvironmentInfo {
    /// Collect environment information for a given path
    pub fn collect_for_path(path: &std::path::Path) -> Result<Self> {
        use crate::utils::{DirectoryStructureUtils, EnvironmentUtils, VcsUtils};

        let platform = EnvironmentUtils::get_platform().to_string();
        let date = EnvironmentUtils::get_current_date();

        let directory_structure = DirectoryStructureUtils::get_directory_structure(
            path,
            MAX_DIRECTORY_DEPTH,
            Some(MAX_DIRECTORY_ITEMS),
        )?;
        debug!("directory_structure: {}", directory_structure);

        let readme_content = EnvironmentUtils::read_readme(path);
        let (memory_file_name, memory_file_content) = match EnvironmentUtils::read_memory_file(path)
        {
            Some((name, content)) => (Some(name), Some(content)),
            None => (None, None),
        };

        Ok(Self {
            working_directory: path.to_path_buf(),
            vcs: VcsUtils::collect_vcs_info(path),
            platform,
            date,
            directory_structure,
            readme_content,
            memory_file_name,
            memory_file_content,
        })
    }

    /// Format environment info as context for system prompt
    pub fn as_context(&self) -> String {
        let vcs_line = match &self.vcs {
            Some(vcs) => format!("VCS: {} ({})", vcs.kind.as_str(), vcs.root.display()),
            None => "VCS: none".to_string(),
        };
        let mut context = format!(
            "Here is useful information about the environment you are running in:\n<env>\nWorking directory: {}\n{}\nPlatform: {}\nToday's date: {}\n</env>",
            self.working_directory.display(),
            vcs_line,
            self.platform,
            self.date
        );

        if !self.directory_structure.is_empty() {
            context.push_str(&format!("\n\n<file_structure>\nBelow is a snapshot of this project's file structure at the start of the conversation. The file structure may be filtered to omit `.gitignore`ed patterns. This snapshot will NOT update during the conversation.\n\n{}\n</file_structure>", self.directory_structure));
        }

        if let Some(ref vcs) = self.vcs {
            context.push_str(&format!(
                "\n<vcs_status>\nThis is the VCS status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.\n\nVCS: {}\nRoot: {}\n\n{}\n</vcs_status>",
                vcs.kind.as_str(),
                vcs.root.display(),
                vcs.status.as_llm_string()
            ));
        }

        if let Some(ref readme) = self.readme_content {
            context.push_str(&format!("\n<file name=\"README.md\">\nThis is the README.md file at the start of the conversation. Note that this README is a snapshot in time, and will not update during the conversation.\n\n{readme}\n</file>"));
        }

        if let (Some(name), Some(content)) = (&self.memory_file_name, &self.memory_file_content) {
            context.push_str(&format!("\n<file name=\"{name}\">\nThis is the {name} file at the start of the conversation. Note that this file is a snapshot in time, and will not update during the conversation.\n\n{content}\n</file>"));
        }

        context
    }
}
