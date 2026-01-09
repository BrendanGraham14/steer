use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ignore::WalkBuilder;
use std::path::Path;

/// Common file listing functionality for workspaces
pub struct FileListingUtils;

impl FileListingUtils {
    /// List files in a directory with optional fuzzy filtering
    pub fn list_files(
        root_path: &Path,
        query: Option<&str>,
        max_results: Option<usize>,
    ) -> Result<Vec<String>, std::io::Error> {
        let mut files = Vec::new();

        // Walk the directory, respecting .gitignore but including hidden files (except VCS dirs)
        let walker = WalkBuilder::new(root_path)
            .hidden(false) // Include hidden files
            .filter_entry(|entry| {
                // Skip VCS directories
                entry.file_name() != ".git" && entry.file_name() != ".jj"
            })
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue, // Skip files we don't have access to
            };

            // Skip the root directory itself
            if entry.path() == root_path {
                continue;
            }

            // Get the relative path from the root
            if let Ok(relative_path) = entry.path().strip_prefix(root_path) {
                if let Some(path_str) = relative_path.to_str() {
                    if !path_str.is_empty() {
                        // Add trailing slash for directories
                        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                            files.push(format!("{path_str}/"));
                        } else {
                            files.push(path_str.to_string());
                        }
                    }
                }
            }
        }

        // Apply fuzzy filter if query is provided
        let mut filtered_files = if let Some(query) = query {
            if query.is_empty() {
                files
            } else {
                let matcher = SkimMatcherV2::default();
                let mut scored_files: Vec<(i64, String)> = files
                    .into_iter()
                    .filter_map(|file| matcher.fuzzy_match(&file, query).map(|score| (score, file)))
                    .collect();

                // Sort by score (highest first)
                scored_files.sort_by(|a, b| b.0.cmp(&a.0));

                scored_files.into_iter().map(|(_, file)| file).collect()
            }
        } else {
            files
        };

        // Apply max_results limit if specified
        if let Some(max) = max_results {
            if max > 0 && filtered_files.len() > max {
                filtered_files.truncate(max);
            }
        }

        Ok(filtered_files)
    }
}

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
        match repo.head_id() {
            Ok(head_id) => {
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
            Err(_) => {}
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
            let parent_commit = repo.store().get_commit(parent_id).map_err(|e| {
                std::io::Error::other(format!("Failed to load parent commit: {e}"))
            })?;
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

/// Common directory structure functionality for workspaces
pub struct DirectoryStructureUtils;

impl DirectoryStructureUtils {
    /// Get directory structure with limited depth and item count
    /// Shows gitignored/hidden directories as leaf nodes with item counts
    pub fn get_directory_structure(
        root_path: &Path,
        max_depth: usize,
        max_items: Option<usize>,
    ) -> Result<String, std::io::Error> {
        let mut structure = vec![root_path.display().to_string()];

        // Use WalkBuilder to respect .gitignore
        let (paths, truncated) = Self::collect_directory_paths(root_path, max_depth, max_items)?;
        structure.extend(paths);

        structure.sort();
        let mut result = structure.join("\n");

        if truncated > 0 {
            result.push_str(&format!("\n... and {truncated} more items"));
        }

        Ok(result)
    }

    /// Collect directory paths respecting .gitignore and filtering hidden directories
    /// Returns (paths, number_of_truncated_items)
    fn collect_directory_paths(
        root_path: &Path,
        max_depth: usize,
        max_items: Option<usize>,
    ) -> Result<(Vec<String>, usize), std::io::Error> {
        let mut paths = Vec::new();
        let mut item_count = 0;
        let mut truncated = 0;
        let limit = max_items.unwrap_or(usize::MAX);
        let mut walker_seen_dirs = std::collections::HashSet::new();

        // First pass: collect allowed entries using WalkBuilder (respects .gitignore)
        // Note: We use hidden(true) to exclude hidden files/dirs from traversal
        let walker = WalkBuilder::new(root_path)
            .max_depth(Some(max_depth))
            .hidden(true) // Exclude hidden files/dirs from traversal
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Skip the root directory itself
            if entry.path() == root_path {
                continue;
            }

            if let Ok(relative_path) = entry.path().strip_prefix(root_path) {
                if let Some(path_str) = relative_path.to_str() {
                    if !path_str.is_empty() {
                        // Track immediate child directories that walker saw
                        if entry.depth() == 1
                            && entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
                        {
                            if let Some(dir_name) = relative_path.file_name() {
                                walker_seen_dirs.insert(dir_name.to_string_lossy().to_string());
                            }
                        }

                        if item_count >= limit {
                            truncated += 1;
                            continue;
                        }

                        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                            paths.push(format!("{path_str}/"));
                        } else {
                            paths.push(path_str.to_string());
                        }
                        item_count += 1;
                    }
                }
            }
        }

        // Second pass: check immediate children for ignored/hidden directories
        // and add them as leaf nodes with counts
        if max_depth > 0 {
            let entries = std::fs::read_dir(root_path)?;
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let file_name = match path.file_name() {
                    Some(name) => name.to_string_lossy().to_string(),
                    None => continue,
                };

                // Skip directories that the walker already saw (even if truncated)
                if walker_seen_dirs.contains(&file_name) {
                    continue;
                }

                // Check if we've reached the limit
                if item_count >= limit {
                    truncated += 1;
                    continue;
                }

                // This is an ignored or hidden directory - count its contents
                let dir_item_count = Self::count_items_in_dir(&path);
                if dir_item_count > 0 {
                    paths.push(format!("{file_name}/ ({dir_item_count} items)"));
                } else {
                    paths.push(format!("{file_name}/ (empty)"));
                }
                item_count += 1;
            }
        }

        Ok((paths, truncated))
    }

    /// Count items in a directory (for ignored/hidden directories)
    fn count_items_in_dir(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| entries.count())
            .unwrap_or(0)
    }
}

/// Common environment utilities for workspaces
pub struct EnvironmentUtils;

impl EnvironmentUtils {
    /// Get the current platform string
    pub fn get_platform() -> &'static str {
        if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "unknown"
        }
    }

    /// Get the current date in YYYY-MM-DD format
    pub fn get_current_date() -> String {
        use chrono::Local;
        Local::now().format("%Y-%m-%d").to_string()
    }

    /// Check if a directory is a git repository
    pub fn is_git_repo(path: &Path) -> bool {
        gix::discover(path).is_ok()
    }

    /// Read README.md if it exists
    pub fn read_readme(path: &Path) -> Option<String> {
        let readme_path = path.join("README.md");
        std::fs::read_to_string(readme_path).ok()
    }

    /// Read AGENTS.md if it exists, otherwise fall back to CLAUDE.md.
    pub fn read_memory_file(path: &Path) -> Option<(String, String)> {
        const PRIMARY_MEMORY_FILE_NAME: &str = "AGENTS.md";
        const FALLBACK_MEMORY_FILE_NAME: &str = "CLAUDE.md";

        let agents_path = path.join(PRIMARY_MEMORY_FILE_NAME);
        if let Ok(content) = std::fs::read_to_string(agents_path) {
            return Some((PRIMARY_MEMORY_FILE_NAME.to_string(), content));
        }

        let claude_path = path.join(FALLBACK_MEMORY_FILE_NAME);
        std::fs::read_to_string(claude_path)
            .ok()
            .map(|content| (FALLBACK_MEMORY_FILE_NAME.to_string(), content))
    }

    /// Read AGENTS.md (preferred) or CLAUDE.md and return only the content.
    pub fn read_claude_md(path: &Path) -> Option<String> {
        Self::read_memory_file(path).map(|(_, content)| content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LlmStatus;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn test_list_files_empty_dir() {
        let temp_dir = tempdir().unwrap();
        let files = FileListingUtils::list_files(temp_dir.path(), None, None).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_list_files_with_content() {
        let temp_dir = tempdir().unwrap();

        // Create some test files
        std::fs::write(temp_dir.path().join("test.rs"), "test").unwrap();
        std::fs::write(temp_dir.path().join("main.rs"), "main").unwrap();
        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        std::fs::write(temp_dir.path().join("src/lib.rs"), "lib").unwrap();

        let files = FileListingUtils::list_files(temp_dir.path(), None, None).unwrap();
        assert_eq!(files.len(), 4); // 3 files + 1 directory
        assert!(files.contains(&"test.rs".to_string()));
        assert!(files.contains(&"main.rs".to_string()));
        assert!(files.contains(&"src/".to_string()));
        assert!(files.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_list_files_with_query() {
        let temp_dir = tempdir().unwrap();
        std::fs::write(temp_dir.path().join("test.rs"), "test").unwrap();
        std::fs::write(temp_dir.path().join("main.rs"), "main").unwrap();

        let files = FileListingUtils::list_files(temp_dir.path(), Some("test"), None).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], "test.rs");
    }

    #[test]
    fn test_platform_detection() {
        let platform = EnvironmentUtils::get_platform();
        assert!(["windows", "macos", "linux", "unknown"].contains(&platform));
    }

    #[test]
    fn test_date_format() {
        let date = EnvironmentUtils::get_current_date();
        // Basic check for YYYY-MM-DD format
        assert_eq!(date.len(), 10);
        assert_eq!(date.chars().nth(4), Some('-'));
        assert_eq!(date.chars().nth(7), Some('-'));
    }

    #[test]
    fn test_git_repo_detection() {
        let temp_dir = tempdir().unwrap();
        assert!(!EnvironmentUtils::is_git_repo(temp_dir.path()));

        // Create a git repo
        gix::init(temp_dir.path()).unwrap();
        assert!(EnvironmentUtils::is_git_repo(temp_dir.path()));
    }

    #[test]
    fn test_vcs_detection_prefers_jj() {
        let temp_dir = tempdir().unwrap();
        std::fs::create_dir(temp_dir.path().join(".git")).unwrap();
        std::fs::create_dir(temp_dir.path().join(".jj")).unwrap();

        let provider = VcsUtils::detect_provider(temp_dir.path()).unwrap();
        assert!(matches!(provider.kind(), crate::VcsKind::Jj));
    }

    #[test]
    fn test_vcs_detection_prefers_closer_git() {
        let (temp_dir, _workspace, _repo) = init_jj_workspace();
        let git_dir = temp_dir.path().join("nested");
        std::fs::create_dir(&git_dir).unwrap();
        gix::init(&git_dir).unwrap();

        let provider = VcsUtils::detect_provider(&git_dir).unwrap();
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
Working copy changes:
<none>
Working copy (@): oxmtprsl 5e7ebcdf (no description set)
Parent commit (@-): zzzzzzzz 00000000 (no description set)
";
        assert_eq!(status.as_llm_string(), expected);
    }

    #[test]
    fn test_jj_status_dirty_after_snapshot() {
        let (temp_dir, _workspace, repo) = init_jj_workspace();
        create_dirty_working_copy(&repo);

        let status = JjStatusUtils::get_jj_status(temp_dir.path()).unwrap();
        let expected = "\
Working copy changes:
A file.txt
Working copy (@): lvxkkpmk ad65a7ea (no description set)
Parent commit (@-): zzzzzzzz 00000000 (no description set)
";
        assert_eq!(status.as_llm_string(), expected);
    }

    #[test]
    #[cfg(unix)]
    fn test_list_files_skips_inaccessible() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempdir().unwrap();

        // Create accessible files
        std::fs::write(temp_dir.path().join("readable.txt"), "test").unwrap();

        // Create an inaccessible directory
        let restricted_dir = temp_dir.path().join("restricted");
        std::fs::create_dir(&restricted_dir).unwrap();
        std::fs::write(restricted_dir.join("hidden.txt"), "secret").unwrap();

        // Remove read permissions from the directory
        let mut perms = std::fs::metadata(&restricted_dir).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&restricted_dir, perms).unwrap();

        // Should list files without error, skipping the inaccessible directory
        let files = FileListingUtils::list_files(temp_dir.path(), None, None).unwrap();

        // Should contain the readable file
        assert!(files.contains(&"readable.txt".to_string()));

        // May or may not contain the restricted directory itself depending on walker behavior
        // but should not error out

        // Restore permissions for cleanup
        let mut perms = std::fs::metadata(&restricted_dir).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&restricted_dir, perms).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn test_directory_structure_skips_inaccessible() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempdir().unwrap();

        // Create accessible directory
        let accessible_dir = temp_dir.path().join("accessible");
        std::fs::create_dir(&accessible_dir).unwrap();
        std::fs::write(accessible_dir.join("file.txt"), "test").unwrap();

        // Create an inaccessible directory
        let restricted_dir = temp_dir.path().join("restricted");
        std::fs::create_dir(&restricted_dir).unwrap();
        std::fs::write(restricted_dir.join("hidden.txt"), "secret").unwrap();

        // Remove read permissions from the directory
        let mut perms = std::fs::metadata(&restricted_dir).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&restricted_dir, perms).unwrap();

        // Should get directory structure without error
        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, None).unwrap();

        // Should contain the accessible directory
        assert!(result.contains("accessible/"));

        // Should not error out due to inaccessible directory

        // Restore permissions for cleanup
        let mut perms = std::fs::metadata(&restricted_dir).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&restricted_dir, perms).unwrap();
    }

    #[test]
    fn test_directory_structure_empty_dir() {
        let temp_dir = tempdir().unwrap();
        let expected = temp_dir.path().display().to_string();
        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_with_gitignored_dirs() {
        let temp_dir = tempdir().unwrap();

        // Create .gitignore file
        std::fs::write(
            temp_dir.path().join(".gitignore"),
            "target/\nnode_modules/\n*.log",
        )
        .unwrap();

        // Create regular files and dirs
        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        std::fs::write(temp_dir.path().join("src/main.rs"), "main").unwrap();
        std::fs::write(temp_dir.path().join("Cargo.toml"), "cargo").unwrap();

        // Create gitignored directories with content
        std::fs::create_dir(temp_dir.path().join("target")).unwrap();
        std::fs::create_dir(temp_dir.path().join("target/debug")).unwrap();
        std::fs::write(temp_dir.path().join("target/debug/app"), "binary").unwrap();

        std::fs::create_dir(temp_dir.path().join("node_modules")).unwrap();
        std::fs::create_dir(temp_dir.path().join("node_modules/pkg1")).unwrap();
        std::fs::create_dir(temp_dir.path().join("node_modules/pkg2")).unwrap();
        std::fs::write(temp_dir.path().join("node_modules/pkg1/index.js"), "js").unwrap();

        // Create .git directory (hidden)
        std::fs::create_dir(temp_dir.path().join(".git")).unwrap();
        std::fs::write(temp_dir.path().join(".git/config"), "config").unwrap();
        std::fs::write(temp_dir.path().join(".git/HEAD"), "HEAD").unwrap();

        // Create gitignored file
        std::fs::write(temp_dir.path().join("debug.log"), "log").unwrap();

        // Build expected output
        // Note: .git is hidden and shown with count
        // .gitignore is excluded as a hidden file with hidden(true)
        let mut expected_lines = [
            temp_dir.path().display().to_string(),
            ".git/ (2 items)".to_string(), // hidden dir, shown with count
            "Cargo.toml".to_string(),
            "node_modules/ (2 items)".to_string(), // gitignored, shown with count
            "src/".to_string(),
            "src/main.rs".to_string(),
            "target/ (1 items)".to_string(), // gitignored, shown with count
        ];
        expected_lines.sort();
        let expected = expected_lines.join("\n");

        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_with_files() {
        let temp_dir = tempdir().unwrap();

        // Create some files
        std::fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(temp_dir.path().join("file2.rs"), "content2").unwrap();

        let mut expected_lines = [
            temp_dir.path().display().to_string(),
            "file1.txt".to_string(),
            "file2.rs".to_string(),
        ];
        expected_lines.sort();
        let expected = expected_lines.join("\n");

        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_with_subdirs() {
        let temp_dir = tempdir().unwrap();

        // Create nested directory structure
        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        std::fs::create_dir(temp_dir.path().join("tests")).unwrap();
        std::fs::write(temp_dir.path().join("src/main.rs"), "main").unwrap();
        std::fs::write(temp_dir.path().join("tests/test.rs"), "test").unwrap();

        let mut expected_lines = [
            temp_dir.path().display().to_string(),
            "src/".to_string(),
            "src/main.rs".to_string(),
            "tests/".to_string(),
            "tests/test.rs".to_string(),
        ];
        expected_lines.sort();
        let expected = expected_lines.join("\n");

        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_max_depth_zero() {
        let temp_dir = tempdir().unwrap();

        // Create nested structure that shouldn't be traversed
        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        std::fs::write(temp_dir.path().join("src/lib.rs"), "lib").unwrap();

        let expected = temp_dir.path().display().to_string();
        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 0, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_max_depth_one() {
        let temp_dir = tempdir().unwrap();

        // Create nested structure
        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        std::fs::create_dir(temp_dir.path().join("src/nested")).unwrap();
        std::fs::write(temp_dir.path().join("file.txt"), "root file").unwrap();
        std::fs::write(temp_dir.path().join("src/lib.rs"), "lib").unwrap();
        std::fs::write(temp_dir.path().join("src/nested/deep.rs"), "deep").unwrap();

        // With max_depth = 1, should get root + immediate children only
        let mut expected_lines = [
            temp_dir.path().display().to_string(),
            "file.txt".to_string(),
            "src/".to_string(),
        ];
        expected_lines.sort();
        let expected = expected_lines.join("\n");

        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 1, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_deeply_nested() {
        let temp_dir = tempdir().unwrap();

        // Create deeply nested structure
        std::fs::create_dir(temp_dir.path().join("a")).unwrap();
        std::fs::create_dir(temp_dir.path().join("a/b")).unwrap();
        std::fs::create_dir(temp_dir.path().join("a/b/c")).unwrap();
        std::fs::write(temp_dir.path().join("a/file1.txt"), "1").unwrap();
        std::fs::write(temp_dir.path().join("a/b/file2.txt"), "2").unwrap();
        std::fs::write(temp_dir.path().join("a/b/c/file3.txt"), "3").unwrap();

        // With max_depth = 2, should get a/ and a/b/ but not a/b/c/
        // Note: a/b/c/ will be detected as a subdirectory and shown with count
        let mut expected_lines = [
            temp_dir.path().display().to_string(),
            "a/".to_string(),
            "a/b/".to_string(),
            "a/file1.txt".to_string(),
        ];
        expected_lines.sort();
        let expected = expected_lines.join("\n");

        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 2, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_mixed_content() {
        let temp_dir = tempdir().unwrap();

        // Create mixed files and directories
        std::fs::write(temp_dir.path().join("README.md"), "readme").unwrap();
        std::fs::write(temp_dir.path().join("Cargo.toml"), "cargo").unwrap();
        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        std::fs::create_dir(temp_dir.path().join("tests")).unwrap();
        std::fs::create_dir(temp_dir.path().join(".git")).unwrap();
        std::fs::write(temp_dir.path().join("src/lib.rs"), "lib").unwrap();
        std::fs::write(temp_dir.path().join("src/main.rs"), "main").unwrap();
        std::fs::write(temp_dir.path().join("tests/integration.rs"), "test").unwrap();
        std::fs::write(temp_dir.path().join(".git/config"), "config").unwrap();

        // .git is not hidden from WalkBuilder with hidden(false), it traverses it
        let mut expected_lines = vec![
            temp_dir.path().display().to_string(),
            ".git/ (1 items)".to_string(), // hidden dir, shown with count
            "Cargo.toml".to_string(),
            "README.md".to_string(),
            "src/".to_string(),
            "src/lib.rs".to_string(),
            "src/main.rs".to_string(),
            "tests/".to_string(),
            "tests/integration.rs".to_string(),
        ];
        expected_lines.sort();
        let expected = expected_lines.join("\n");

        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_with_hidden_files() {
        let temp_dir = tempdir().unwrap();

        // Create some regular and hidden files/directories
        std::fs::write(temp_dir.path().join("README.md"), "readme").unwrap();
        std::fs::write(temp_dir.path().join(".env"), "secrets").unwrap(); // hidden file
        std::fs::write(temp_dir.path().join(".gitignore"), "*.log").unwrap(); // hidden file

        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        std::fs::write(temp_dir.path().join("src/main.rs"), "main").unwrap();

        std::fs::create_dir(temp_dir.path().join(".cache")).unwrap(); // hidden dir
        std::fs::write(temp_dir.path().join(".cache/data"), "cached").unwrap();

        std::fs::create_dir(temp_dir.path().join(".hidden")).unwrap(); // hidden dir
        std::fs::create_dir(temp_dir.path().join(".hidden/nested")).unwrap();
        std::fs::write(temp_dir.path().join(".hidden/file.txt"), "hidden").unwrap();

        // Build expected output
        // Hidden directories shown with counts, hidden files excluded by hidden(true)
        let mut expected_lines = [
            temp_dir.path().display().to_string(),
            ".cache/ (1 items)".to_string(), // hidden dir with count
            // .env and .gitignore are excluded by hidden(true)
            ".hidden/ (2 items)".to_string(), // hidden dir with count
            "README.md".to_string(),
            "src/".to_string(),
            "src/main.rs".to_string(),
        ];
        expected_lines.sort();
        let expected = expected_lines.join("\n");

        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_special_chars() {
        let temp_dir = tempdir().unwrap();

        // Create files with special characters
        std::fs::write(temp_dir.path().join("file with spaces.txt"), "content").unwrap();
        std::fs::write(temp_dir.path().join("file-with-dashes.rs"), "content").unwrap();
        std::fs::write(temp_dir.path().join("file_with_underscores.md"), "content").unwrap();
        std::fs::create_dir(temp_dir.path().join("dir with spaces")).unwrap();

        let mut expected_lines = [
            temp_dir.path().display().to_string(),
            "dir with spaces/".to_string(),
            "file with spaces.txt".to_string(),
            "file-with-dashes.rs".to_string(),
            "file_with_underscores.md".to_string(),
        ];
        expected_lines.sort();
        let expected = expected_lines.join("\n");

        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, None).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_with_max_items_limit() {
        let temp_dir = tempdir().unwrap();

        // Create 20 files
        for i in 0..20 {
            std::fs::write(temp_dir.path().join(format!("file{i:02}.txt")), "content").unwrap();
        }

        // Test with limit of 5 items
        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, Some(5)).unwrap();

        let lines: Vec<&str> = result.lines().collect();

        // Verify structure
        assert_eq!(lines[0], temp_dir.path().display().to_string());
        assert_eq!(lines.len(), 7); // root + 5 items + truncation
        assert_eq!(lines[6], "... and 15 more items");

        // Verify we got 5 files (can't predict which ones due to traversal order)
        for line in lines.iter().take(6).skip(1) {
            assert!(line.ends_with(".txt"));
        }
    }

    #[test]
    fn test_directory_structure_with_dirs_and_max_items() {
        let temp_dir = tempdir().unwrap();

        // Create 5 items
        std::fs::create_dir(temp_dir.path().join("dir1")).unwrap();
        std::fs::create_dir(temp_dir.path().join("dir2")).unwrap();
        std::fs::write(temp_dir.path().join("file1.txt"), "content").unwrap();
        std::fs::write(temp_dir.path().join("file2.txt"), "content").unwrap();
        std::fs::create_dir(temp_dir.path().join("dir3")).unwrap();

        // Test with limit of 3 items
        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, Some(3)).unwrap();

        let expected = format!(
            "{}\ndir2/\nfile1.txt\nfile2.txt\n... and 2 more items",
            temp_dir.path().display()
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_directory_structure_no_truncation_when_under_limit() {
        let temp_dir = tempdir().unwrap();

        // Create just a few files
        std::fs::write(temp_dir.path().join("file1.txt"), "content").unwrap();
        std::fs::write(temp_dir.path().join("file2.txt"), "content").unwrap();
        std::fs::create_dir(temp_dir.path().join("subdir")).unwrap();

        // Test with high limit
        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, Some(100))
                .unwrap();

        // Should not have truncation message
        assert!(!result.contains("... and"));
        assert!(!result.contains("more items"));

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 4); // root + 2 files + 1 dir
    }

    #[test]
    fn test_directory_structure_with_hidden_dirs_and_limit() {
        let temp_dir = tempdir().unwrap();

        // Create regular files
        for i in 0..5 {
            std::fs::write(temp_dir.path().join(format!("file{i}.txt")), "content").unwrap();
        }

        // Create hidden directories
        std::fs::create_dir(temp_dir.path().join(".hidden1")).unwrap();
        std::fs::write(temp_dir.path().join(".hidden1/file.txt"), "hidden").unwrap();

        std::fs::create_dir(temp_dir.path().join(".hidden2")).unwrap();
        std::fs::write(temp_dir.path().join(".hidden2/file.txt"), "hidden").unwrap();

        // Test with limit of 4 items
        let result =
            DirectoryStructureUtils::get_directory_structure(temp_dir.path(), 3, Some(4)).unwrap();

        let lines: Vec<&str> = result.lines().collect();

        // Verify structure
        assert_eq!(lines[0], temp_dir.path().display().to_string());
        assert_eq!(lines.len(), 6); // root + 4 items + truncation
        assert_eq!(lines[5], "... and 3 more items");

        // The walker sees the 5 regular files (not hidden dirs), picks first 4 in traversal order
        // Hidden dirs are only seen by the second pass, but we've already hit the limit
        for line in lines.iter().take(5).skip(1) {
            assert!(line.ends_with(".txt"));
        }
    }
}
