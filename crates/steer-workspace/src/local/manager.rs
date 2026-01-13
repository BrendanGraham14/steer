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
use crate::workspace_registry::WorkspaceRegistry;
use crate::{
    EnvironmentId, RepoId, RepoInfo, VcsKind, Workspace, WorkspaceId, WorkspaceInfo,
    WorkspaceStatus,
};

use super::git;
use super::jj;
use super::layout::WorkspaceLayout;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct LocalWorkspaceManager {
    layout: WorkspaceLayout,
    environment_id: EnvironmentId,
    registry: Arc<WorkspaceRegistry>,
}

impl LocalWorkspaceManager {
    pub async fn new(root: PathBuf) -> WorkspaceManagerResult<Self> {
        let registry = WorkspaceRegistry::open(&root).await?;
        let manager = Self {
            layout: WorkspaceLayout::new(root),
            environment_id: EnvironmentId::local(),
            registry: Arc::new(registry),
        };
        Ok(manager)
    }

    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    fn repo_id_for_path(&self, path: &Path) -> RepoId {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let uuid = Uuid::new_v5(&Uuid::NAMESPACE_URL, canonical.to_string_lossy().as_bytes());
        RepoId::from_uuid(uuid)
    }
}

#[async_trait]
impl WorkspaceManager for LocalWorkspaceManager {
    async fn resolve_workspace(&self, path: &Path) -> WorkspaceManagerResult<WorkspaceInfo> {
        let vcs_info = VcsUtils::collect_vcs_info(path)
            .ok_or_else(|| WorkspaceManagerError::NotSupported("No VCS detected".to_string()))?;

        match vcs_info.kind {
            VcsKind::Jj => {
                let workspace_root = vcs_info.root;
                if let Some(existing) = self.registry.find_by_path(&workspace_root).await? {
                    return Ok(existing);
                }

                let (workspace, _repo) = jj::load_jj_workspace(&workspace_root)?;
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
            VcsKind::Git => {
                let workspace_root = vcs_info.root;
                if let Some(existing) = self.registry.find_by_path(&workspace_root).await? {
                    return Ok(existing);
                }

                let repo_id = git::repo_id_for_path(&workspace_root)?;
                let repo_info = if let Some(existing) = self.registry.fetch_repo(repo_id).await? {
                    existing
                } else {
                    let info = RepoInfo {
                        repo_id,
                        environment_id: self.environment_id,
                        root_path: workspace_root.clone(),
                        vcs_kind: Some(VcsKind::Git),
                    };
                    self.registry.upsert_repo(&info).await?;
                    info
                };

                let info = WorkspaceInfo {
                    workspace_id: WorkspaceId::new(),
                    environment_id: self.environment_id,
                    repo_id: repo_info.repo_id,
                    parent_workspace_id: None,
                    name: self.layout.default_workspace_name_for_path(&workspace_root),
                    path: workspace_root,
                };
                self.registry.insert_workspace(&info).await?;

                Ok(info)
            }
        }
    }

    async fn create_workspace(
        &self,
        request: CreateWorkspaceRequest,
    ) -> WorkspaceManagerResult<WorkspaceInfo> {
        match request.strategy {
            WorkspaceCreateStrategy::JjWorkspace => {
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

                let jj_root = jj::ensure_jj_workspace_root(&repo_info.root_path)?;
                let workspace_id = WorkspaceId::new();
                let requested_name = request
                    .name
                    .unwrap_or_else(|| self.layout.default_workspace_name(workspace_id));
                let jj_name = self.layout.sanitize_name(&requested_name);

                let parent_dir = self.layout.workspace_parent_dir(repo_info.repo_id);
                std::fs::create_dir_all(&parent_dir)?;
                let workspace_path = self.layout.ensure_unique_path(&parent_dir, &jj_name);
                std::fs::create_dir_all(&workspace_path)?;

                {
                    let (workspace, repo) = jj::load_jj_workspace(&jj_root)?;
                    jj::ensure_workspace_name_available(&repo, &jj_name)?;

                    jj::init_workspace_with_existing_repo(
                        &workspace_path,
                        workspace.repo_path(),
                        &repo,
                        jj_name.as_str(),
                    )?;
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
            WorkspaceCreateStrategy::GitWorktree => {
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
                if repo_info.vcs_kind != Some(VcsKind::Git) {
                    return Err(WorkspaceManagerError::NotSupported(
                        "Git worktree creation is only supported for git repositories".to_string(),
                    ));
                }

                let workspace_id = WorkspaceId::new();
                let requested_name = request
                    .name
                    .unwrap_or_else(|| self.layout.default_workspace_name(workspace_id));
                let sanitized_name = self.layout.sanitize_name(&requested_name);
                let parent_dir = self.layout.workspace_parent_dir(repo_info.repo_id);
                std::fs::create_dir_all(&parent_dir)?;
                let workspace_path = self.layout.ensure_unique_path(&parent_dir, &sanitized_name);
                let names = git::worktree_names(
                    &self.layout,
                    workspace_id,
                    &sanitized_name,
                    &workspace_path,
                );

                git::create_worktree(
                    &self.layout,
                    &repo_info.root_path,
                    &workspace_path,
                    &names.worktree_name,
                    &names.branch_name,
                )?;
                let workspace_path =
                    std::fs::canonicalize(&workspace_path).unwrap_or(workspace_path);

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
        }
    }

    async fn list_workspaces(
        &self,
        request: ListWorkspacesRequest,
    ) -> WorkspaceManagerResult<Vec<WorkspaceInfo>> {
        if request.environment_id != self.environment_id {
            return Err(WorkspaceManagerError::NotFound(
                request.environment_id.as_uuid().to_string(),
            ));
        }
        self.registry.list_workspaces(request.environment_id).await
    }

    async fn open_workspace(
        &self,
        workspace_id: WorkspaceId,
    ) -> WorkspaceManagerResult<Arc<dyn Workspace>> {
        let info = self
            .registry
            .fetch_workspace(workspace_id)
            .await?
            .ok_or_else(|| WorkspaceManagerError::NotFound(workspace_id.as_uuid().to_string()))?;

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
            .ok_or_else(|| WorkspaceManagerError::NotFound(workspace_id.as_uuid().to_string()))?;

        if let Ok(jj_root) = jj::ensure_jj_workspace_root(&info.path)
            && let Ok((mut workspace, repo)) = jj::load_jj_workspace(&jj_root)
        {
            let _ = jj::snapshot_working_copy(&mut workspace, repo).await;
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
        let repo_info = self
            .registry
            .fetch_repo(info.repo_id)
            .await?
            .ok_or_else(|| {
                WorkspaceManagerError::NotFound(format!(
                    "Repo not found: {}",
                    info.repo_id.as_uuid()
                ))
            })?;

        let managed_root = self.layout.workspace_parent_dir(info.repo_id);
        let managed_root =
            std::fs::canonicalize(&managed_root).unwrap_or_else(|_| managed_root.clone());
        let info_path = std::fs::canonicalize(&info.path).unwrap_or_else(|_| info.path.clone());
        if !info_path.starts_with(&managed_root) {
            return Err(WorkspaceManagerError::InvalidRequest(
                "Only managed workspaces can be deleted".to_string(),
            ));
        }

        match repo_info.vcs_kind {
            Some(VcsKind::Jj) => {
                let jj_root = jj::ensure_jj_workspace_root(&info.path)?;
                let (mut workspace, repo) = jj::load_jj_workspace(&jj_root)?;
                let workspace_name = workspace.workspace_name().to_owned();
                let repo = jj::snapshot_working_copy(&mut workspace, repo).await?;
                let mut tx = repo.start_transaction();
                tx.repo_mut()
                    .remove_wc_commit(&workspace_name)
                    .map_err(|e| {
                        WorkspaceManagerError::Other(format!("Failed to remove jj workspace: {e}"))
                    })?;
                if tx.repo_mut().has_rewrites() {
                    tx.repo_mut().rebase_descendants().map_err(|e| {
                        WorkspaceManagerError::Other(format!(
                            "Failed to rebase jj descendants after workspace removal: {e}"
                        ))
                    })?;
                }
                let workspace_name_ref: &jj_lib::ref_name::WorkspaceName = workspace_name.as_ref();
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
            Some(VcsKind::Git) => {
                git::remove_worktree(&repo_info.root_path, &info.path)?;
                if info.path.exists() {
                    std::fs::remove_dir_all(&info.path)?;
                }
            }
            None => {
                return Err(WorkspaceManagerError::NotSupported(
                    "Workspace orchestration requires a VCS".to_string(),
                ));
            }
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
                let repo_id = git::repo_id_for_path(&repo_root)?;
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
                let (workspace, _repo) = jj::load_jj_workspace(&workspace_root)?;
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
mod tests {
    use super::*;
    use jj_lib::repo::Repo;
    use jj_lib::repo_path::RepoPathBuf;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_no_default_workspace_registered() {
        let temp = TempDir::new().unwrap();
        let manager = LocalWorkspaceManager::new(temp.path().to_path_buf())
            .await
            .unwrap();

        let workspaces = manager
            .list_workspaces(ListWorkspacesRequest {
                environment_id: manager.environment_id(),
            })
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

    #[test]
    fn test_delete_workspace_snapshots_and_preserves_revision() {
        let temp_dir = tempfile::tempdir().unwrap();
        let settings = {
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
        };
        let (workspace, _repo) =
            jj_lib::workspace::Workspace::init_simple(&settings, temp_dir.path()).unwrap();
        let repo_path = workspace.repo_path();
        let config_path = repo_path.join("config.toml");
        std::fs::write(config_path, r#"snapshot.auto-track = "all()""#).unwrap();
        drop(workspace);

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let manager_root = tempfile::tempdir().unwrap();
        let manager = runtime
            .block_on(LocalWorkspaceManager::new(
                manager_root.path().to_path_buf(),
            ))
            .unwrap();

        let base_info = runtime
            .block_on(manager.resolve_workspace(temp_dir.path()))
            .unwrap();
        let child_info = runtime
            .block_on(manager.create_workspace(CreateWorkspaceRequest {
                repo_id: base_info.repo_id,
                name: Some("subagent-test".to_string()),
                parent_workspace_id: Some(base_info.workspace_id),
                strategy: WorkspaceCreateStrategy::JjWorkspace,
            }))
            .unwrap();

        assert!(child_info.path.exists());
        let managed_root =
            std::fs::canonicalize(manager.layout.workspace_parent_dir(child_info.repo_id))
                .unwrap_or_else(|_| manager.layout.workspace_parent_dir(child_info.repo_id));
        let child_path =
            std::fs::canonicalize(&child_info.path).unwrap_or_else(|_| child_info.path.clone());
        assert!(
            child_path.starts_with(&managed_root),
            "workspace path should be managed (path: {child_path:?}, managed_root: {managed_root:?})"
        );
        std::fs::write(child_info.path.join("subagent.txt"), "content").unwrap();

        runtime
            .block_on(manager.delete_workspace(DeleteWorkspaceRequest {
                workspace_id: child_info.workspace_id,
            }))
            .unwrap();

        assert!(!child_info.path.exists());

        let (_workspace, repo) = jj::load_jj_workspace(temp_dir.path()).unwrap();
        let repo_path = RepoPathBuf::from_internal_string("subagent.txt").unwrap();
        let found = repo.view().heads().iter().any(|commit_id| {
            let commit = repo.store().get_commit(commit_id).unwrap();
            commit
                .tree()
                .unwrap()
                .path_value(repo_path.as_ref())
                .unwrap()
                .is_present()
        });

        assert!(
            found,
            "snapshot commit should remain after workspace cleanup"
        );
    }

    #[tokio::test]
    async fn test_git_worktree_create_and_delete() {
        let temp_dir = TempDir::new().unwrap();
        let repo_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let repo = gix::init(&repo_root).unwrap();
        let signature = gix::actor::Signature {
            name: "Test User".into(),
            email: "test@example.com".into(),
            time: gix::actor::date::Time::default(),
        };
        let mut time_buf = gix::actor::date::parse::TimeBuf::default();
        let sig_ref = signature.to_ref(&mut time_buf);
        let head_id = repo
            .commit_as(
                sig_ref,
                sig_ref,
                "HEAD",
                "initial",
                repo.empty_tree().id,
                Vec::<gix::ObjectId>::new(),
            )
            .unwrap();
        let head_oid = head_id.detach();

        let manager_root = TempDir::new().unwrap();
        let manager = LocalWorkspaceManager::new(manager_root.path().to_path_buf())
            .await
            .unwrap();

        let base = manager.resolve_workspace(&repo_root).await.unwrap();
        let created = manager
            .create_workspace(CreateWorkspaceRequest {
                repo_id: base.repo_id,
                name: Some("git-subagent".to_string()),
                parent_workspace_id: Some(base.workspace_id),
                strategy: WorkspaceCreateStrategy::GitWorktree,
            })
            .await
            .unwrap();

        assert!(created.path.exists());
        let gitfile = created.path.join(".git");
        assert!(gitfile.is_file());
        let private_git_dir = gix::discover::path::from_gitdir_file(&gitfile).unwrap();
        assert!(private_git_dir.is_dir());
        assert!(private_git_dir.join("refs").is_dir());

        let orig_head = std::fs::read_to_string(private_git_dir.join("ORIG_HEAD")).unwrap();
        assert_eq!(orig_head.trim(), head_oid.to_string());
        let head_log = std::fs::read_to_string(private_git_dir.join("logs").join("HEAD")).unwrap();
        assert!(head_log.contains(&head_oid.to_string()));
        assert!(head_log.contains("checkout: moving from (initial)"));

        let worktree_repo = gix::discover(&created.path).unwrap();
        assert!(matches!(
            worktree_repo.kind(),
            gix::repository::Kind::WorkTree { is_linked: true }
        ));

        let vcs_info = VcsUtils::collect_vcs_info(&created.path).unwrap();
        assert_eq!(vcs_info.kind, VcsKind::Git);
        if let crate::VcsStatus::Git(status) = vcs_info.status {
            assert!(status.error.is_none());
        } else {
            panic!("expected git status");
        }

        let main_repo = gix::discover(&repo_root).unwrap();
        let worktrees = main_repo.worktrees().unwrap();
        let mut listed = false;
        for proxy in worktrees {
            if let Ok(base) = proxy.base()
                && base == created.path
            {
                listed = true;
                break;
            }
        }
        assert!(listed, "worktree should be listed");

        manager
            .delete_workspace(DeleteWorkspaceRequest {
                workspace_id: created.workspace_id,
            })
            .await
            .unwrap();

        assert!(!created.path.exists());
        assert!(!private_git_dir.exists());
    }

    #[tokio::test]
    async fn test_git_worktree_delete_when_worktree_missing() {
        let temp_dir = TempDir::new().unwrap();
        let repo_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let repo = gix::init(&repo_root).unwrap();
        let signature = gix::actor::Signature {
            name: "Test User".into(),
            email: "test@example.com".into(),
            time: gix::actor::date::Time::default(),
        };
        let mut time_buf = gix::actor::date::parse::TimeBuf::default();
        let sig_ref = signature.to_ref(&mut time_buf);
        repo.commit_as(
            sig_ref,
            sig_ref,
            "HEAD",
            "initial",
            repo.empty_tree().id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();

        let manager_root = TempDir::new().unwrap();
        let manager = LocalWorkspaceManager::new(manager_root.path().to_path_buf())
            .await
            .unwrap();

        let base = manager.resolve_workspace(&repo_root).await.unwrap();
        let created = manager
            .create_workspace(CreateWorkspaceRequest {
                repo_id: base.repo_id,
                name: Some("git-subagent-missing".to_string()),
                parent_workspace_id: Some(base.workspace_id),
                strategy: WorkspaceCreateStrategy::GitWorktree,
            })
            .await
            .unwrap();

        let gitfile = created.path.join(".git");
        let private_git_dir = gix::discover::path::from_gitdir_file(&gitfile).unwrap();

        std::fs::remove_dir_all(&created.path).unwrap();

        manager
            .delete_workspace(DeleteWorkspaceRequest {
                workspace_id: created.workspace_id,
            })
            .await
            .unwrap();

        assert!(!private_git_dir.exists());
    }
}
