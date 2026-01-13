use std::path::Path;

/// Common git status functionality for workspaces
pub struct GitStatusUtils;

impl GitStatusUtils {
    /// Get git status information for a repository
    pub fn get_git_status(repo_path: &Path) -> Result<crate::GitStatus, std::io::Error> {
        let repo = gix::discover(repo_path)
            .map_err(|e| std::io::Error::other(format!("Failed to open git repository: {e}")))?;

        // Get current branch
        let head = match repo.head_name() {
            Ok(Some(name)) => {
                let branch = name.as_bstr().to_string();
                let branch = branch.strip_prefix("refs/heads/").unwrap_or(&branch);
                crate::GitHead::Branch(branch.to_string())
            }
            Ok(None) => crate::GitHead::Detached,
            Err(e) => {
                if e.to_string().contains("does not exist") {
                    crate::GitHead::Unborn
                } else {
                    return Err(std::io::Error::other(format!("Failed to get HEAD: {e}")));
                }
            }
        };

        // Get status
        let iter = repo
            .status(gix::progress::Discard)
            .map_err(|e| std::io::Error::other(format!("Failed to get git status: {e}")))?
            .into_index_worktree_iter(Vec::new())
            .map_err(|e| std::io::Error::other(format!("Failed to get git status: {e}")))?;
        use gix::bstr::ByteSlice;
        use gix::status::index_worktree::iter::Summary;
        let mut entries = Vec::new();
        for item_res in iter {
            let item = item_res
                .map_err(|e| std::io::Error::other(format!("Failed to get git status: {e}")))?;
            if let Some(summary) = item.summary() {
                let path = item.rela_path().to_str_lossy();
                let summary = match summary {
                    Summary::Added => crate::GitStatusSummary::Added,
                    Summary::Removed => crate::GitStatusSummary::Removed,
                    Summary::Modified => crate::GitStatusSummary::Modified,
                    Summary::TypeChange => crate::GitStatusSummary::TypeChange,
                    Summary::Renamed => crate::GitStatusSummary::Renamed,
                    Summary::Copied => crate::GitStatusSummary::Copied,
                    Summary::IntentToAdd => crate::GitStatusSummary::IntentToAdd,
                    Summary::Conflict => crate::GitStatusSummary::Conflict,
                };
                entries.push(crate::GitStatusEntry {
                    summary,
                    path: path.to_string(),
                });
            }
        }

        // Get recent commits
        let mut recent_commits = Vec::new();
        if let Ok(head_id) = repo.head_id() {
            let oid = head_id.detach();
            if let Ok(object) = repo.find_object(oid) {
                if let Ok(commit) = object.try_into_commit() {
                    // Just show the HEAD commit for now, as rev_walk API changed
                    let summary_bytes = commit.message_raw_sloppy();
                    let summary = summary_bytes
                        .lines()
                        .next()
                        .and_then(|line| std::str::from_utf8(line).ok())
                        .unwrap_or("<no summary>");
                    let short_id = oid.to_hex().to_string();
                    let short_id = &short_id[..7.min(short_id.len())];
                    recent_commits.push(crate::GitCommitSummary {
                        id: short_id.to_string(),
                        summary: summary.to_string(),
                    });
                }
            }
        }

        Ok(crate::GitStatus::new(head, entries, recent_commits))
    }
}

trait VcsProvider {
    fn kind(&self) -> crate::VcsKind;
    fn root(&self) -> &Path;
    fn status(&self) -> Result<crate::VcsStatus, std::io::Error>;
}

struct GitProvider {
    root: std::path::PathBuf,
}

impl VcsProvider for GitProvider {
    fn kind(&self) -> crate::VcsKind {
        crate::VcsKind::Git
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn status(&self) -> Result<crate::VcsStatus, std::io::Error> {
        GitStatusUtils::get_git_status(&self.root).map(crate::VcsStatus::Git)
    }
}

struct JjProvider {
    root: std::path::PathBuf,
}

impl VcsProvider for JjProvider {
    fn kind(&self) -> crate::VcsKind {
        crate::VcsKind::Jj
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn status(&self) -> Result<crate::VcsStatus, std::io::Error> {
        JjStatusUtils::get_jj_status(&self.root).map(crate::VcsStatus::Jj)
    }
}

/// Common VCS detection and status functionality
pub struct VcsUtils;

impl VcsUtils {
    pub fn collect_vcs_info(path: &Path) -> Option<crate::VcsInfo> {
        let provider = Self::detect_provider(path)?;
        let status = match provider.status() {
            Ok(status) => status,
            Err(err) => match provider.kind() {
                crate::VcsKind::Git => {
                    crate::VcsStatus::Git(crate::GitStatus::unavailable(err.to_string()))
                }
                crate::VcsKind::Jj => {
                    crate::VcsStatus::Jj(crate::JjStatus::unavailable(err.to_string()))
                }
            },
        };
        Some(crate::VcsInfo {
            kind: provider.kind(),
            root: provider.root().to_path_buf(),
            status,
        })
    }

    fn detect_provider(path: &Path) -> Option<Box<dyn VcsProvider>> {
        let jj_root = Self::find_marker_root(path, ".jj");
        let git_root = Self::find_git_root(path);

        match (jj_root, git_root) {
            (Some(jj_root), Some(git_root)) => {
                let jj_distance = Self::distance_from(path, &jj_root);
                let git_distance = Self::distance_from(path, &git_root);

                match (jj_distance, git_distance) {
                    (Some(jj_distance), Some(git_distance)) => {
                        if jj_distance < git_distance {
                            Some(Box::new(JjProvider { root: jj_root }))
                        } else if git_distance < jj_distance {
                            Some(Box::new(GitProvider { root: git_root }))
                        } else {
                            Some(Box::new(JjProvider { root: jj_root }))
                        }
                    }
                    (Some(_), None) => Some(Box::new(JjProvider { root: jj_root })),
                    (None, Some(_)) => Some(Box::new(GitProvider { root: git_root })),
                    (None, None) => Some(Box::new(JjProvider { root: jj_root })),
                }
            }
            (Some(jj_root), None) => Some(Box::new(JjProvider { root: jj_root })),
            (None, Some(git_root)) => Some(Box::new(GitProvider { root: git_root })),
            (None, None) => None,
        }
    }

    fn find_marker_root(path: &Path, marker: &str) -> Option<std::path::PathBuf> {
        let mut current = Some(path);
        while let Some(dir) = current {
            if dir.join(marker).is_dir() {
                return Some(dir.to_path_buf());
            }
            current = dir.parent();
        }
        None
    }

    fn find_git_root(path: &Path) -> Option<std::path::PathBuf> {
        let repo = gix::discover(path).ok()?;
        let root = repo.workdir().unwrap_or_else(|| repo.path());
        Some(root.to_path_buf())
    }

    fn distance_from(path: &Path, root: &Path) -> Option<usize> {
        let relative = path.strip_prefix(root).ok()?;
        Some(relative.components().count())
    }

    #[cfg(test)]
    fn detect_provider_for_tests(path: &Path) -> Option<Box<dyn VcsProvider>> {
        Self::detect_provider(path)
    }
}

struct JjStatusUtils;

impl JjStatusUtils {
    pub fn get_jj_status(workspace_root: &Path) -> Result<crate::JjStatus, std::io::Error> {
        use jj_lib::config::{ConfigSource, StackedConfig};
        use jj_lib::matchers::EverythingMatcher;
        use jj_lib::object_id::ObjectId;
        use jj_lib::repo::{Repo, StoreFactories};
        use jj_lib::settings::UserSettings;
        use jj_lib::workspace::{Workspace, default_working_copy_factories};

        let mut config = StackedConfig::with_defaults();
        let jj_dir = workspace_root.join(".jj");
        let repo_config = jj_dir.join("repo").join("config.toml");
        if repo_config.is_file() {
            config
                .load_file(ConfigSource::Repo, repo_config)
                .map_err(|e| {
                    std::io::Error::other(format!("Failed to load jj repo config: {e}"))
                })?;
        }
        let workspace_config = jj_dir.join("workspace-config.toml");
        if workspace_config.is_file() {
            config
                .load_file(ConfigSource::Workspace, workspace_config)
                .map_err(|e| {
                    std::io::Error::other(format!("Failed to load jj workspace config: {e}"))
                })?;
        }

        let settings = UserSettings::from_config(config)
            .map_err(|e| std::io::Error::other(format!("Failed to build jj settings: {e}")))?;
        let store_factories = StoreFactories::default();
        let working_copy_factories = default_working_copy_factories();
        let workspace = Workspace::load(
            &settings,
            workspace_root,
            &store_factories,
            &working_copy_factories,
        )
        .map_err(|e| std::io::Error::other(format!("Failed to load jj workspace: {e}")))?;
        let repo = workspace
            .repo_loader()
            .load_at_head()
            .map_err(|e| std::io::Error::other(format!("Failed to load jj repo: {e}")))?;

        let workspace_name = workspace.workspace_name();
        let wc_commit_id = repo
            .view()
            .get_wc_commit_id(workspace_name)
            .ok_or_else(|| {
                std::io::Error::other(format!(
                    "No working copy commit for workspace '{}'",
                    workspace_name.as_str()
                ))
            })?;
        let wc_commit = repo.store().get_commit(wc_commit_id).map_err(|e| {
            std::io::Error::other(format!("Failed to load working copy commit: {e}"))
        })?;

        let parent_tree = wc_commit
            .parent_tree(repo.as_ref())
            .map_err(|e| std::io::Error::other(format!("Failed to load parent tree: {e}")))?;
        let wc_tree = wc_commit
            .tree()
            .map_err(|e| std::io::Error::other(format!("Failed to load working copy tree: {e}")))?;
        let changes = Self::collect_changes(&parent_tree, &wc_tree, &EverythingMatcher)?;

        let change_id_full = wc_commit.change_id().reverse_hex();
        let change_id = short_id(&change_id_full).to_string();
        let commit_id_full = wc_commit.id().hex();
        let commit_id = short_id(&commit_id_full).to_string();
        let description = first_line(wc_commit.description()).trim();
        let description = if description.is_empty() {
            "(no description set)".to_string()
        } else {
            description.to_string()
        };

        let working_copy = crate::JjCommitSummary {
            change_id,
            commit_id,
            description,
        };

        let mut parents = Vec::new();
        let parent_ids = wc_commit.parent_ids();
        for parent_id in parent_ids.iter() {
            let parent_commit = repo
                .store()
                .get_commit(parent_id)
                .map_err(|e| std::io::Error::other(format!("Failed to load parent commit: {e}")))?;
            let parent_change_id_full = parent_commit.change_id().reverse_hex();
            let parent_change_id = short_id(&parent_change_id_full).to_string();
            let parent_commit_id_full = parent_commit.id().hex();
            let parent_commit_id = short_id(&parent_commit_id_full).to_string();
            let parent_description = first_line(parent_commit.description()).trim();
            let parent_description = if parent_description.is_empty() {
                "(no description set)".to_string()
            } else {
                parent_description.to_string()
            };
            parents.push(crate::JjCommitSummary {
                change_id: parent_change_id,
                commit_id: parent_commit_id,
                description: parent_description,
            });
        }

        Ok(crate::JjStatus::new(changes, working_copy, parents))
    }

    fn collect_changes(
        parent_tree: &jj_lib::merged_tree::MergedTree,
        wc_tree: &jj_lib::merged_tree::MergedTree,
        matcher: &jj_lib::matchers::EverythingMatcher,
    ) -> Result<Vec<crate::JjChange>, std::io::Error> {
        let mut changes = Vec::new();
        for entry in jj_lib::merged_tree::TreeDiffIterator::new(
            parent_tree.as_merge(),
            wc_tree.as_merge(),
            matcher,
        ) {
            let diff = entry.values.map_err(|e| {
                std::io::Error::other(format!("Failed to diff working copy changes: {e}"))
            })?;
            if !diff.is_changed() {
                continue;
            }
            let change_type = if diff.before.is_absent() && diff.after.is_present() {
                crate::JjChangeType::Added
            } else if diff.before.is_present() && diff.after.is_absent() {
                crate::JjChangeType::Removed
            } else {
                crate::JjChangeType::Modified
            };
            changes.push(crate::JjChange {
                change_type,
                path: entry.path.as_internal_file_string().to_string(),
            });
        }
        changes.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(changes)
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

fn short_id(hex: &str) -> &str {
    let len = hex.len().min(8);
    &hex[..len]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LlmStatus;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn test_vcs_detection_prefers_jj() {
        let temp_dir = tempdir().unwrap();
        std::fs::create_dir(temp_dir.path().join(".git")).unwrap();
        std::fs::create_dir(temp_dir.path().join(".jj")).unwrap();

        let provider = VcsUtils::detect_provider_for_tests(temp_dir.path()).unwrap();
        assert!(matches!(provider.kind(), crate::VcsKind::Jj));
    }

    #[test]
    fn test_vcs_detection_prefers_closer_git() {
        let (temp_dir, _workspace, _repo) = init_jj_workspace();
        let git_dir = temp_dir.path().join("nested");
        std::fs::create_dir(&git_dir).unwrap();
        gix::init(&git_dir).unwrap();

        let provider = VcsUtils::detect_provider_for_tests(&git_dir).unwrap();
        assert!(matches!(provider.kind(), crate::VcsKind::Git));
    }

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
        tempfile::TempDir,
        jj_lib::workspace::Workspace,
        Arc<jj_lib::repo::ReadonlyRepo>,
    ) {
        let temp_dir = tempdir().unwrap();
        let settings = jj_settings();
        let (workspace, repo) =
            jj_lib::workspace::Workspace::init_simple(&settings, temp_dir.path()).unwrap();
        (temp_dir, workspace, repo)
    }

    fn create_dirty_working_copy(repo: &Arc<jj_lib::repo::ReadonlyRepo>) {
        use jj_lib::backend::{CopyId, TreeValue};
        use jj_lib::merge::Merge;
        use jj_lib::merged_tree::MergedTreeBuilder;
        use jj_lib::ref_name::WorkspaceName;
        use jj_lib::repo::Repo;
        use jj_lib::repo_path::RepoPathBuf;
        use std::io::Cursor;

        let workspace_name = WorkspaceName::DEFAULT;
        let wc_commit_id = repo.view().get_wc_commit_id(workspace_name).unwrap();
        let wc_commit = repo.store().get_commit(wc_commit_id).unwrap();

        let file_path = RepoPathBuf::from_internal_string("file.txt").unwrap();
        let mut contents = Cursor::new(b"content".to_vec());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let file_id = runtime
            .block_on(repo.store().write_file(file_path.as_ref(), &mut contents))
            .unwrap();

        let mut tree_builder = MergedTreeBuilder::new(repo.store().empty_merged_tree_id());
        tree_builder.set_or_remove(
            file_path,
            Merge::normal(TreeValue::File {
                id: file_id,
                executable: false,
                copy_id: CopyId::new(vec![]),
            }),
        );
        let new_tree_id = tree_builder.write_tree(repo.store()).unwrap();

        let mut tx = repo.start_transaction();
        let new_commit = tx
            .repo_mut()
            .new_commit(wc_commit.parent_ids().to_vec(), new_tree_id)
            .write()
            .unwrap();
        tx.repo_mut()
            .set_wc_commit(workspace_name.to_owned(), new_commit.id().clone())
            .unwrap();
        tx.commit("test dirty working copy").unwrap();
    }

    #[test]
    fn test_jj_status_clean() {
        let (temp_dir, _workspace, _repo) = init_jj_workspace();

        let status = JjStatusUtils::get_jj_status(temp_dir.path()).unwrap();
        let expected = "\
Working copy changes:\n<none>\nWorking copy (@): oxmtprsl 5e7ebcdf (no description set)\nParent commit (@-): zzzzzzzz 00000000 (no description set)\n";
        assert_eq!(status.as_llm_string(), expected);
    }

    #[test]
    fn test_jj_status_dirty_after_snapshot() {
        let (temp_dir, _workspace, repo) = init_jj_workspace();
        create_dirty_working_copy(&repo);

        let status = JjStatusUtils::get_jj_status(temp_dir.path()).unwrap();
        let expected = "\
Working copy changes:\nA file.txt\nWorking copy (@): lvxkkpmk ad65a7ea (no description set)\nParent commit (@-): zzzzzzzz 00000000 (no description set)\n";
        assert_eq!(status.as_llm_string(), expected);
    }
}
