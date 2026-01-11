use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::VcsKind;
use crate::error::{WorkspaceManagerError, WorkspaceManagerResult};
use crate::utils::VcsUtils;

use jj_lib::config::ConfigGetError;
use jj_lib::file_util;
use jj_lib::fileset::{self, FilesetDiagnostics};
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::Matcher;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPathUiConverter;
use jj_lib::settings::HumanByteSize;
use jj_lib::working_copy::SnapshotOptions;
use jj_lib::workspace::{Workspace as JjWorkspace, default_working_copy_factory};

const DEFAULT_MAX_NEW_FILE_SIZE: u64 = 10_000_000;

pub(crate) fn ensure_jj_workspace_root(path: &Path) -> WorkspaceManagerResult<PathBuf> {
    let vcs_info = VcsUtils::collect_vcs_info(path)
        .ok_or_else(|| WorkspaceManagerError::NotSupported("No VCS detected".to_string()))?;

    match vcs_info.kind {
        VcsKind::Jj => Ok(vcs_info.root),
        VcsKind::Git => Err(WorkspaceManagerError::NotSupported(
            "Workspace orchestration is disabled for git repositories".to_string(),
        )),
    }
}

pub(crate) fn load_jj_settings(
    workspace_root: &Path,
) -> WorkspaceManagerResult<jj_lib::settings::UserSettings> {
    use jj_lib::config::{ConfigSource, StackedConfig};

    let mut config = StackedConfig::with_defaults();
    let jj_dir = workspace_root.join(".jj");
    let repo_config = {
        let repo_dir = jj_dir.join("repo");
        if repo_dir.is_file() {
            let buf = std::fs::read(&repo_dir).map_err(|e| {
                WorkspaceManagerError::Other(format!("Failed to read jj repo path file: {e}"))
            })?;
            let repo_path = file_util::path_from_bytes(&buf).map_err(|e| {
                WorkspaceManagerError::Other(format!("Failed to decode jj repo path: {e}"))
            })?;
            let repo_dir = std::fs::canonicalize(jj_dir.join(repo_path)).map_err(|e| {
                WorkspaceManagerError::Other(format!("Failed to resolve jj repo path: {e}"))
            })?;
            repo_dir.join("config.toml")
        } else {
            repo_dir.join("config.toml")
        }
    };
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
                WorkspaceManagerError::Other(format!("Failed to load jj workspace config: {e}"))
            })?;
    }

    jj_lib::settings::UserSettings::from_config(config)
        .map_err(|e| WorkspaceManagerError::Other(format!("Failed to build jj settings: {e}")))
}

pub(crate) fn load_jj_workspace(
    workspace_root: &Path,
) -> WorkspaceManagerResult<(JjWorkspace, Arc<jj_lib::repo::ReadonlyRepo>)> {
    use jj_lib::repo::StoreFactories;
    use jj_lib::workspace::default_working_copy_factories;

    let settings = load_jj_settings(workspace_root)?;
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

pub(crate) async fn snapshot_working_copy(
    workspace: &mut JjWorkspace,
    repo: Arc<jj_lib::repo::ReadonlyRepo>,
) -> WorkspaceManagerResult<Arc<jj_lib::repo::ReadonlyRepo>> {
    let workspace_name = workspace.workspace_name().to_owned();
    let start_tracking_matcher = snapshot_start_tracking_matcher(workspace)?;
    let max_new_file_size = snapshot_max_new_file_size(workspace.settings())?;
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
            WorkspaceManagerError::Other(format!("Failed to update jj working copy state: {e}"))
        })?;

    Ok(updated_repo)
}

pub(crate) fn ensure_workspace_name_available(
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

pub(crate) fn init_workspace_with_existing_repo(
    workspace_path: &Path,
    repo_path: &Path,
    repo: &Arc<jj_lib::repo::ReadonlyRepo>,
    name: &str,
) -> WorkspaceManagerResult<()> {
    let working_copy_factory = default_working_copy_factory();
    JjWorkspace::init_workspace_with_existing_repo(
        workspace_path,
        repo_path,
        repo,
        &*working_copy_factory,
        WorkspaceNameBuf::from(name),
    )
    .map_err(|e| WorkspaceManagerError::Other(format!("Failed to create jj workspace: {e}")))?;
    Ok(())
}

fn snapshot_start_tracking_matcher(
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
    let fileset = fileset::parse(&mut diagnostics, expression, &path_converter).map_err(|err| {
        WorkspaceManagerError::Other(format!(
            "Failed to parse jj snapshot.auto-track fileset: {err}"
        ))
    })?;
    Ok(fileset.to_matcher())
}

fn snapshot_max_new_file_size(
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

#[cfg(test)]
mod tests {
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
        write_repo_config(&workspace, r#"snapshot.auto-track = "none()""#);
        drop(workspace);

        std::fs::write(temp_dir.path().join("new.txt"), "content").unwrap();

        let (mut workspace, repo) = load_jj_workspace(temp_dir.path()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let updated_repo = runtime
            .block_on(snapshot_working_copy(&mut workspace, repo))
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

        let (mut workspace, repo) = load_jj_workspace(temp_dir.path()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let updated_repo = runtime
            .block_on(snapshot_working_copy(&mut workspace, repo))
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

        let (mut workspace, repo) = load_jj_workspace(temp_dir.path()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let updated_repo = runtime
            .block_on(snapshot_working_copy(&mut workspace, repo))
            .unwrap();

        assert!(
            !wc_tree_has_path(&updated_repo, workspace.workspace_name(), "large.txt"),
            "large file should remain untracked when it exceeds snapshot.max-new-file-size"
        );
    }
}
