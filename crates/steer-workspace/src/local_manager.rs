use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{WorkspaceManagerError, WorkspaceManagerResult};
use crate::local::LocalWorkspace;
use crate::manager::{
    CreateWorkspaceRequest, DeleteWorkspaceRequest, ListWorkspacesRequest, WorkspaceCreateStrategy,
    WorkspaceManager,
};
use crate::utils::VcsUtils;
use crate::{
    EnvironmentId, RepoId, RepoInfo, VcsKind, Workspace, WorkspaceId, WorkspaceInfo,
    WorkspaceStatus,
};
use crate::workspace_registry::WorkspaceRegistry;

use jj_lib::config::ConfigGetError;
use jj_lib::fileset::{self, FilesetDiagnostics};
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::Matcher;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPathUiConverter;
use jj_lib::ref_name::WorkspaceNameBuf;
use jj_lib::settings::HumanByteSize;
use jj_lib::workspace::{Workspace as JjWorkspace, default_working_copy_factory};
use jj_lib::working_copy::SnapshotOptions;
use uuid::Uuid;

const DEFAULT_MAX_NEW_FILE_SIZE: u64 = 10_000_000;

#[derive(Debug, Clone)]
pub struct LocalWorkspaceManager {
    root: PathBuf,
    environment_id: EnvironmentId,
    registry: Arc<WorkspaceRegistry>,
}

impl LocalWorkspaceManager {
    pub async fn new(root: PathBuf) -> WorkspaceManagerResult<Self> {
        let registry = WorkspaceRegistry::open(&root).await?;
        let manager = Self {
            root,
            environment_id: EnvironmentId::local(),
            registry: Arc::new(registry),
        };
        Ok(manager)
    }

    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    fn ensure_jj_workspace_root(&self, path: &Path) -> WorkspaceManagerResult<std::path::PathBuf> {
        let vcs_info = VcsUtils::collect_vcs_info(path)
            .ok_or_else(|| WorkspaceManagerError::NotSupported("No VCS detected".to_string()))?;

        match vcs_info.kind {
            VcsKind::Jj => Ok(vcs_info.root),
            VcsKind::Git => Err(WorkspaceManagerError::NotSupported(
                "Workspace orchestration is disabled for git repositories".to_string(),
            )),
        }
    }

    fn workspace_parent_dir(&self, repo_id: RepoId) -> PathBuf {
        self.root
            .join(".steer")
            .join("workspaces")
            .join(repo_id.as_uuid().to_string())
    }

    fn sanitize_name(&self, name: &str) -> String {
        let mut sanitized = String::new();
        for ch in name.chars() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                sanitized.push(ch);
            } else if ch.is_ascii_whitespace() {
                sanitized.push('-');
            }
        }
        if sanitized.is_empty() {
            "workspace".to_string()
        } else {
            sanitized
        }
    }

    fn default_workspace_name(&self, workspace_id: WorkspaceId) -> String {
        let id = workspace_id.as_uuid().to_string();
        let short = id.split('-').next().unwrap_or("workspace");
        format!("ws-{short}")
    }

    fn ensure_unique_path(&self, base_dir: &Path, name: &str) -> PathBuf {
        let mut candidate = base_dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
        let mut counter = 1;
        loop {
            let candidate_name = format!("{name}-{counter}");
            candidate = base_dir.join(candidate_name);
            if !candidate.exists() {
                return candidate;
            }
            counter += 1;
        }
    }

    fn load_jj_settings(&self, workspace_root: &Path) -> WorkspaceManagerResult<jj_lib::settings::UserSettings> {
        use jj_lib::config::{ConfigSource, StackedConfig};

        let mut config = StackedConfig::with_defaults();
        let jj_dir = workspace_root.join(".jj");
        let repo_config = jj_dir.join("repo").join("config.toml");
        if repo_config.is_file() {
            config
                .load_file(ConfigSource::Repo, repo_config)
                .map_err(|e| {
                    WorkspaceManagerError::Other(format!("Failed to load jj repo config: {e}"))
                })?;
        }
        let workspace_config = jj_dir.join("workspace-config.toml");
        if workspace_config.is_file() {
            config
                .load_file(ConfigSource::Workspace, workspace_config)
                .map_err(|e| {
                    WorkspaceManagerError::Other(format!(
                        "Failed to load jj workspace config: {e}"
                    ))
                })?;
        }

        jj_lib::settings::UserSettings::from_config(config).map_err(|e| {
            WorkspaceManagerError::Other(format!("Failed to build jj settings: {e}"))
        })
    }

    fn load_jj_workspace(
        &self,
        workspace_root: &Path,
    ) -> WorkspaceManagerResult<(JjWorkspace, Arc<jj_lib::repo::ReadonlyRepo>)> {
        use jj_lib::repo::StoreFactories;
        use jj_lib::workspace::default_working_copy_factories;

        let settings = self.load_jj_settings(workspace_root)?;
        let store_factories = StoreFactories::default();
        let working_copy_factories = default_working_copy_factories();
        let workspace = JjWorkspace::load(
            &settings,
            workspace_root,
            &store_factories,
            &working_copy_factories,
        )
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to load jj workspace: {e}")))?;
        let repo = workspace
            .repo_loader()
            .load_at_head()
            .map_err(|e| WorkspaceManagerError::Other(format!("Failed to load jj repo: {e}")))?;
        Ok((workspace, repo))
    }

    async fn snapshot_working_copy(
        &self,
        workspace: &mut JjWorkspace,
        repo: Arc<jj_lib::repo::ReadonlyRepo>,
    ) -> WorkspaceManagerResult<Arc<jj_lib::repo::ReadonlyRepo>> {
        let workspace_name = workspace.workspace_name().to_owned();
        let start_tracking_matcher = self.snapshot_start_tracking_matcher(workspace)?;
        let max_new_file_size = self.snapshot_max_new_file_size(workspace.settings())?;
        let mut locked_ws = workspace.start_working_copy_mutation().map_err(|e| {
            WorkspaceManagerError::Other(format!("Failed to lock jj working copy: {e}"))
        })?;
        let old_tree_id = locked_ws.locked_wc().old_tree_id().clone();
        let old_op_id = locked_ws.locked_wc().old_operation_id().clone();

        let snapshot_options = SnapshotOptions {
            base_ignores: GitIgnoreFile::empty(),
            progress: None,
            start_tracking_matcher: start_tracking_matcher.as_ref(),
            max_new_file_size,
        };

        let snapshot_result = locked_ws.locked_wc().snapshot(&snapshot_options).await;
        let (new_tree_id, _stats) = match snapshot_result {
            Ok(result) => result,
            Err(err) => {
                locked_ws.finish(old_op_id).map_err(|e| {
                    WorkspaceManagerError::Other(format!(
                        "Failed to release jj working copy after snapshot error: {e}"
                    ))
                })?;
                return Err(WorkspaceManagerError::Other(format!(
                    "Failed to snapshot jj working copy: {err}"
                )));
            }
        };

        let mut updated_repo = repo.clone();
        if new_tree_id != old_tree_id {
            let wc_commit_id = repo
                .view()
                .get_wc_commit_id(workspace_name.as_ref())
                .ok_or_else(|| {
                    WorkspaceManagerError::Other(format!(
                        "No working copy commit for workspace '{}'",
                        workspace_name.as_str()
                    ))
                })?;
            let wc_commit = repo.store().get_commit(wc_commit_id).map_err(|e| {
                WorkspaceManagerError::Other(format!("Failed to load working copy commit: {e}"))
            })?;

            let mut tx = repo.start_transaction();
            let new_commit = tx
                .repo_mut()
                .new_commit(wc_commit.parent_ids().to_vec(), new_tree_id)
                .write()
                .map_err(|e| {
                    WorkspaceManagerError::Other(format!("Failed to write jj snapshot commit: {e}"))
                })?;
            tx.repo_mut()
                .set_wc_commit(workspace_name, new_commit.id().clone())
                .map_err(|e| {
                    WorkspaceManagerError::Other(format!(
                        "Failed to update jj working copy commit: {e}"
                    ))
                })?;

            updated_repo = tx
                .commit("snapshot working copy for workspace cleanup")
                .map_err(|e| {
                    WorkspaceManagerError::Other(format!(
                        "Failed to commit jj snapshot transaction: {e}"
                    ))
                })?;
        }

        locked_ws
            .finish(updated_repo.op_id().clone())
            .map_err(|e| {
                WorkspaceManagerError::Other(format!(
                    "Failed to update jj working copy state: {e}"
                ))
            })?;

        Ok(updated_repo)
    }

    fn snapshot_start_tracking_matcher(
        &self,
        workspace: &JjWorkspace,
    ) -> WorkspaceManagerResult<Box<dyn Matcher>> {
        let settings = workspace.settings();
        let expression = match settings.get_string("snapshot.auto-track") {
            Ok(value) => value,
            Err(ConfigGetError::NotFound { .. }) => String::new(),
            Err(err) => {
                return Err(WorkspaceManagerError::Other(format!(
                    "Failed to read jj snapshot.auto-track setting: {err}"
                )));
            }
        };

        let expression = if expression.trim().is_empty() {
            "none()"
        } else {
            expression.trim()
        };
        let path_converter = RepoPathUiConverter::Fs {
            cwd: workspace.workspace_root().to_path_buf(),
            base: workspace.workspace_root().to_path_buf(),
        };
        let mut diagnostics = FilesetDiagnostics::new();
        let fileset = fileset::parse(&mut diagnostics, expression, &path_converter).map_err(
            |err| {
                WorkspaceManagerError::Other(format!(
                    "Failed to parse jj snapshot.auto-track fileset: {err}"
                ))
            },
        )?;
        Ok(fileset.to_matcher())
    }

    fn snapshot_max_new_file_size(
        &self,
        settings: &jj_lib::settings::UserSettings,
    ) -> WorkspaceManagerResult<u64> {
        match settings.get_value("snapshot.max-new-file-size") {
            Ok(value) => HumanByteSize::try_from(value)
                .map(|size| size.0)
                .map_err(|err| {
                    WorkspaceManagerError::Other(format!(
                        "Invalid jj snapshot.max-new-file-size setting: {err}"
                    ))
                }),
            Err(ConfigGetError::NotFound { .. }) => Ok(DEFAULT_MAX_NEW_FILE_SIZE),
            Err(err) => Err(WorkspaceManagerError::Other(format!(
                "Failed to read jj snapshot.max-new-file-size setting: {err}"
            ))),
        }
    }

    fn repo_id_for_path(&self, path: &Path) -> RepoId {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let uuid = Uuid::new_v5(&Uuid::NAMESPACE_URL, canonical.to_string_lossy().as_bytes());
        RepoId::from_uuid(uuid)
    }

    fn ensure_workspace_name_available(
        &self,
        repo: &Arc<jj_lib::repo::ReadonlyRepo>,
        name: &str,
    ) -> WorkspaceManagerResult<()> {
        let workspace_name = WorkspaceNameBuf::from(name);
        if repo.view().wc_commit_ids().contains_key(&workspace_name) {
            return Err(WorkspaceManagerError::InvalidRequest(format!(
                "Workspace name already exists: {name}"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl WorkspaceManager for LocalWorkspaceManager {
    async fn resolve_workspace(
        &self,
        path: &Path,
    ) -> WorkspaceManagerResult<WorkspaceInfo> {
        let workspace_root = self.ensure_jj_workspace_root(path)?;
        if let Some(existing) = self.registry.find_by_path(&workspace_root).await? {
            return Ok(existing);
        }

        let (workspace, _repo) = self.load_jj_workspace(&workspace_root)?;
        let repo_path = workspace.repo_path().to_path_buf();
        let repo_id = self.repo_id_for_path(&repo_path);

        let repo_info = if let Some(existing) = self.registry.fetch_repo(repo_id).await? {
            existing
        } else {
            let info = RepoInfo {
                repo_id,
                environment_id: self.environment_id,
                root_path: workspace_root.clone(),
                vcs_kind: Some(VcsKind::Jj),
            };
            self.registry.upsert_repo(&info).await?;
            info
        };

        let workspace_name = workspace.workspace_name().as_str().to_string();
        let info = WorkspaceInfo {
            workspace_id: WorkspaceId::new(),
            environment_id: self.environment_id,
            repo_id: repo_info.repo_id,
            parent_workspace_id: None,
            name: Some(workspace_name),
            path: workspace_root,
        };
        self.registry.insert_workspace(&info).await?;

        Ok(info)
    }

    async fn create_workspace(
        &self,
        request: CreateWorkspaceRequest,
    ) -> WorkspaceManagerResult<WorkspaceInfo> {
        if !matches!(request.strategy, WorkspaceCreateStrategy::JjWorkspace) {
            return Err(WorkspaceManagerError::NotSupported(
                "Only jj workspace creation is supported".to_string(),
            ));
        }

        let repo_info = self
            .registry
            .fetch_repo(request.repo_id)
            .await?
            .ok_or_else(|| {
                WorkspaceManagerError::NotFound(format!(
                    "Repo not found: {}",
                    request.repo_id.as_uuid()
                ))
            })?;
        if repo_info.vcs_kind != Some(VcsKind::Jj) {
            return Err(WorkspaceManagerError::NotSupported(
                "Workspace orchestration is only supported for jj repositories".to_string(),
            ));
        }

        let jj_root = self.ensure_jj_workspace_root(&repo_info.root_path)?;
        let workspace_id = WorkspaceId::new();
        let requested_name =
            request.name.unwrap_or_else(|| self.default_workspace_name(workspace_id));
        let jj_name = self.sanitize_name(&requested_name);

        let parent_dir = self.workspace_parent_dir(repo_info.repo_id);
        std::fs::create_dir_all(&parent_dir)?;
        let workspace_path = self.ensure_unique_path(&parent_dir, &jj_name);
        std::fs::create_dir_all(&workspace_path)?;

        {
            let (workspace, repo) = self.load_jj_workspace(&jj_root)?;
            self.ensure_workspace_name_available(&repo, &jj_name)?;

            let working_copy_factory = default_working_copy_factory();
            let _ = JjWorkspace::init_workspace_with_existing_repo(
                &workspace_path,
                workspace.repo_path(),
                &repo,
                &*working_copy_factory,
                WorkspaceNameBuf::from(jj_name.as_str()),
            )
            .map_err(|e| {
                WorkspaceManagerError::Other(format!("Failed to create jj workspace: {e}"))
            })?;
        }

        let info = WorkspaceInfo {
            workspace_id,
            environment_id: self.environment_id,
            repo_id: repo_info.repo_id,
            parent_workspace_id: request.parent_workspace_id,
            name: Some(requested_name),
            path: workspace_path,
        };
        self.registry.insert_workspace(&info).await?;

        Ok(info)
    }

    async fn list_workspaces(
        &self,
        _request: ListWorkspacesRequest,
    ) -> WorkspaceManagerResult<Vec<WorkspaceInfo>> {
        self.registry
            .list_workspaces(self.environment_id)
            .await
    }

    async fn open_workspace(
        &self,
        workspace_id: WorkspaceId,
    ) -> WorkspaceManagerResult<Arc<dyn Workspace>> {
        let info = self
            .registry
            .fetch_workspace(workspace_id)
            .await?
            .ok_or_else(|| {
                WorkspaceManagerError::NotFound(workspace_id.as_uuid().to_string())
            })?;

        let workspace = LocalWorkspace::with_path(info.path.clone())
            .await
            .map_err(|e| WorkspaceManagerError::Other(e.to_string()))?;
        Ok(Arc::new(workspace))
    }

    async fn get_workspace_status(
        &self,
        workspace_id: WorkspaceId,
    ) -> WorkspaceManagerResult<WorkspaceStatus> {
        let info = self
            .registry
            .fetch_workspace(workspace_id)
            .await?
            .ok_or_else(|| {
                WorkspaceManagerError::NotFound(workspace_id.as_uuid().to_string())
            })?;

        if let Ok(jj_root) = self.ensure_jj_workspace_root(&info.path) {
            if let Ok((mut workspace, repo)) = self.load_jj_workspace(&jj_root) {
                let _ = self.snapshot_working_copy(&mut workspace, repo).await;
            }
        }

        let vcs = VcsUtils::collect_vcs_info(&info.path);
        Ok(WorkspaceStatus {
            workspace_id: info.workspace_id,
            environment_id: info.environment_id,
            repo_id: info.repo_id,
            path: info.path,
            vcs,
        })
    }

    async fn delete_workspace(
        &self,
        request: DeleteWorkspaceRequest,
    ) -> WorkspaceManagerResult<()> {
        let info = self
            .registry
            .fetch_workspace(request.workspace_id)
            .await?
            .ok_or_else(|| {
                WorkspaceManagerError::NotFound(request.workspace_id.as_uuid().to_string())
            })?;

        {
            let managed_root = self.workspace_parent_dir(info.repo_id);
            if !info.path.starts_with(&managed_root) {
                return Err(WorkspaceManagerError::InvalidRequest(
                    "Only managed jj workspaces can be deleted".to_string(),
                ));
            }

            let jj_root = self.ensure_jj_workspace_root(&info.path)?;
            let (mut workspace, repo) = self.load_jj_workspace(&jj_root)?;
            let workspace_name = workspace.workspace_name().to_owned();
            let repo = self.snapshot_working_copy(&mut workspace, repo).await?;
            let mut tx = repo.start_transaction();
            tx.repo_mut()
                .remove_wc_commit(&workspace_name)
                .map_err(|e| {
                    WorkspaceManagerError::Other(format!(
                        "Failed to remove jj workspace: {e}"
                    ))
                })?;
            if tx.repo_mut().has_rewrites() {
                tx.repo_mut().rebase_descendants().map_err(|e| {
                    WorkspaceManagerError::Other(format!(
                        "Failed to rebase jj descendants after workspace removal: {e}"
                    ))
                })?;
            }
            let workspace_name_ref: &jj_lib::ref_name::WorkspaceName =
                workspace_name.as_ref();
            tx.commit(format!(
                "forget workspace '{}'",
                workspace_name_ref.as_str()
            ))
            .map_err(|e| {
                WorkspaceManagerError::Other(format!(
                    "Failed to commit jj workspace removal: {e}"
                ))
            })?;

            std::fs::remove_dir_all(&info.path)?;
        }

        self.registry.delete_workspace(request.workspace_id).await?;

        Ok(())
    }
}

#[async_trait]
impl crate::manager::RepoManager for LocalWorkspaceManager {
    async fn resolve_repo(
        &self,
        environment_id: EnvironmentId,
        path: &Path,
    ) -> WorkspaceManagerResult<RepoInfo> {
        if environment_id != self.environment_id {
            return Err(WorkspaceManagerError::NotFound(
                environment_id.as_uuid().to_string(),
            ));
        }

        let vcs_info = VcsUtils::collect_vcs_info(path)
            .ok_or_else(|| WorkspaceManagerError::NotSupported("No VCS detected".to_string()))?;

        match vcs_info.kind {
            VcsKind::Git => {
                let repo_root = vcs_info.root;
                let repo_id = self.repo_id_for_path(&repo_root);
                if let Some(existing) = self.registry.fetch_repo(repo_id).await? {
                    return Ok(existing);
                }
                let info = RepoInfo {
                    repo_id,
                    environment_id: self.environment_id,
                    root_path: repo_root,
                    vcs_kind: Some(VcsKind::Git),
                };
                self.registry.upsert_repo(&info).await?;
                Ok(info)
            }
            VcsKind::Jj => {
                let workspace_root = vcs_info.root;
                let (workspace, _repo) = self.load_jj_workspace(&workspace_root)?;
                let repo_path = workspace.repo_path().to_path_buf();
                let repo_id = self.repo_id_for_path(&repo_path);
                if let Some(existing) = self.registry.fetch_repo(repo_id).await? {
                    return Ok(existing);
                }
                let info = RepoInfo {
                    repo_id,
                    environment_id: self.environment_id,
                    root_path: workspace_root,
                    vcs_kind: Some(VcsKind::Jj),
                };
                self.registry.upsert_repo(&info).await?;
                Ok(info)
            }
        }
    }

    async fn list_repos(
        &self,
        environment_id: EnvironmentId,
    ) -> WorkspaceManagerResult<Vec<RepoInfo>> {
        if environment_id != self.environment_id {
            return Err(WorkspaceManagerError::NotFound(
                environment_id.as_uuid().to_string(),
            ));
        }
        self.registry.list_repos(environment_id).await
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use jj_lib::repo::Repo;
    use jj_lib::repo_path::RepoPathBuf;
    use tempfile::TempDir;

    fn jj_settings() -> jj_lib::settings::UserSettings {
        let mut config = jj_lib::config::StackedConfig::with_defaults();
        let overrides = jj_lib::config::ConfigLayer::parse(
            jj_lib::config::ConfigSource::CommandArg,
            r#"
user.name = "Test User"
user.email = "test@example.com"
operation.hostname = "test-host"
operation.username = "test-user"
signing.behavior = "drop"
debug.randomness-seed = 0
debug.commit-timestamp = "2001-01-01T00:00:00Z"
debug.operation-timestamp = "2001-01-01T00:00:00Z"
"#,
        )
        .unwrap();
        config.add_layer(overrides);
        jj_lib::settings::UserSettings::from_config(config).unwrap()
    }

    fn init_jj_workspace() -> (
        TempDir,
        jj_lib::workspace::Workspace,
        Arc<jj_lib::repo::ReadonlyRepo>,
    ) {
        let temp_dir = tempfile::tempdir().unwrap();
        let settings = jj_settings();
        let (workspace, repo) =
            jj_lib::workspace::Workspace::init_simple(&settings, temp_dir.path()).unwrap();
        (temp_dir, workspace, repo)
    }

    fn write_repo_config(workspace: &jj_lib::workspace::Workspace, contents: &str) {
        let repo_path = workspace.repo_path();
        let config_path = repo_path.join("config.toml");
        std::fs::write(config_path, contents).unwrap();
    }

    fn wc_tree_has_path(
        repo: &Arc<jj_lib::repo::ReadonlyRepo>,
        workspace_name: &jj_lib::ref_name::WorkspaceName,
        path: &str,
    ) -> bool {
        let wc_commit_id = repo.view().get_wc_commit_id(workspace_name).unwrap();
        let wc_commit = repo.store().get_commit(wc_commit_id).unwrap();
        let tree = wc_commit.tree().unwrap();
        let repo_path = RepoPathBuf::from_internal_string(path).unwrap();
        tree.path_value(repo_path.as_ref()).unwrap().is_present()
    }

    #[test]
    fn test_snapshot_auto_track_none_does_not_track_new_files() {
        let (temp_dir, workspace, _repo) = init_jj_workspace();
        write_repo_config(
            &workspace,
            r#"snapshot.auto-track = "none()""#,
        );
        drop(workspace);

        std::fs::write(temp_dir.path().join("new.txt"), "content").unwrap();

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let manager_root = tempfile::tempdir().unwrap();
        let manager = runtime
            .block_on(LocalWorkspaceManager::new(manager_root.path().to_path_buf()))
            .unwrap();
        let (mut workspace, repo) = manager.load_jj_workspace(temp_dir.path()).unwrap();
        let updated_repo = runtime
            .block_on(manager.snapshot_working_copy(&mut workspace, repo))
            .unwrap();

        assert!(
            !wc_tree_has_path(&updated_repo, workspace.workspace_name(), "new.txt"),
            "new file should remain untracked when snapshot.auto-track is none()"
        );
    }

    #[test]
    fn test_snapshot_auto_track_all_tracks_new_files() {
        let (temp_dir, workspace, _repo) = init_jj_workspace();
        write_repo_config(&workspace, r#"snapshot.auto-track = "all()""#);
        drop(workspace);

        std::fs::write(temp_dir.path().join("tracked.txt"), "content").unwrap();

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let manager_root = tempfile::tempdir().unwrap();
        let manager = runtime
            .block_on(LocalWorkspaceManager::new(manager_root.path().to_path_buf()))
            .unwrap();
        let (mut workspace, repo) = manager.load_jj_workspace(temp_dir.path()).unwrap();
        let updated_repo = runtime
            .block_on(manager.snapshot_working_copy(&mut workspace, repo))
            .unwrap();

        assert!(
            wc_tree_has_path(&updated_repo, workspace.workspace_name(), "tracked.txt"),
            "new file should be tracked when snapshot.auto-track is all()"
        );
    }

    #[test]
    fn test_snapshot_max_new_file_size_blocks_large_file() {
        let (temp_dir, workspace, _repo) = init_jj_workspace();
        write_repo_config(
            &workspace,
            r#"
snapshot.auto-track = "all()"
snapshot.max-new-file-size = "1B"
"#,
        );
        drop(workspace);

        std::fs::write(temp_dir.path().join("large.txt"), "ab").unwrap();

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let manager_root = tempfile::tempdir().unwrap();
        let manager = runtime
            .block_on(LocalWorkspaceManager::new(manager_root.path().to_path_buf()))
            .unwrap();
        let (mut workspace, repo) = manager.load_jj_workspace(temp_dir.path()).unwrap();
        let updated_repo = runtime
            .block_on(manager.snapshot_working_copy(&mut workspace, repo))
            .unwrap();

        assert!(
            !wc_tree_has_path(&updated_repo, workspace.workspace_name(), "large.txt"),
            "large file should remain untracked when it exceeds snapshot.max-new-file-size"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_no_default_workspace_registered() {
        let temp = TempDir::new().unwrap();
        let manager = LocalWorkspaceManager::new(temp.path().to_path_buf())
            .await
            .unwrap();

        let workspaces = manager
            .list_workspaces(ListWorkspacesRequest {})
            .await
            .unwrap();
        assert!(workspaces.is_empty());
    }

    #[tokio::test]
    async fn test_create_workspace_requires_jj_repo() {
        let temp = TempDir::new().unwrap();
        let manager = LocalWorkspaceManager::new(temp.path().to_path_buf())
            .await
            .unwrap();

        let result = manager
            .create_workspace(CreateWorkspaceRequest {
                repo_id: RepoId::new(),
                name: Some("child".to_string()),
                parent_workspace_id: None,
                strategy: WorkspaceCreateStrategy::JjWorkspace,
            })
            .await;

        assert!(matches!(result, Err(WorkspaceManagerError::NotFound(_))));
    }
}
