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

        // Walk the directory, respecting .gitignore but including hidden files
        let walker = WalkBuilder::new(root_path)
            .hidden(false) // Include hidden files
            .build();

        for entry in walker {
            let entry = entry.map_err(|e| {
                std::io::Error::other(format!("Failed to read directory entry: {e}"))
            })?;

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
    pub fn get_git_status(repo_path: &Path) -> Result<String, std::io::Error> {
        use git2::Repository;

        let mut result = String::new();

        let repo = Repository::discover(repo_path)
            .map_err(|e| std::io::Error::other(format!("Failed to open git repository: {e}")))?;

        // Get current branch
        match repo.head() {
            Ok(head) => {
                let branch = if head.is_branch() {
                    head.shorthand().unwrap_or("<unknown>")
                } else {
                    "HEAD (detached)"
                };
                result.push_str(&format!("Current branch: {branch}\n\n"));
            }
            Err(e) => {
                // Handle case where HEAD doesn't exist (new repo)
                if e.code() == git2::ErrorCode::UnbornBranch {
                    result.push_str("Current branch: <unborn>\n\n");
                } else {
                    return Err(std::io::Error::other(format!("Failed to get HEAD: {e}")));
                }
            }
        }

        // Get status
        let statuses = repo
            .statuses(None)
            .map_err(|e| std::io::Error::other(format!("Failed to get git status: {e}")))?;
        result.push_str("Status:\n");
        if statuses.is_empty() {
            result.push_str("Working tree clean\n");
        } else {
            for entry in statuses.iter() {
                let status = entry.status();
                let path = entry.path().unwrap_or("<unknown>");

                let status_char = if status.contains(git2::Status::INDEX_NEW) {
                    "A"
                } else if status.contains(git2::Status::INDEX_MODIFIED) {
                    "M"
                } else if status.contains(git2::Status::INDEX_DELETED) {
                    "D"
                } else if status.contains(git2::Status::WT_NEW) {
                    "?"
                } else if status.contains(git2::Status::WT_MODIFIED) {
                    "M"
                } else if status.contains(git2::Status::WT_DELETED) {
                    "D"
                } else {
                    " "
                };

                let wt_char = if status.contains(git2::Status::WT_NEW) {
                    "?"
                } else if status.contains(git2::Status::WT_MODIFIED) {
                    "M"
                } else if status.contains(git2::Status::WT_DELETED) {
                    "D"
                } else {
                    " "
                };

                result.push_str(&format!("{status_char}{wt_char} {path}\n"));
            }
        }

        // Get recent commits
        result.push_str("\nRecent commits:\n");
        match repo.revwalk() {
            Ok(mut revwalk) => {
                if let Ok(()) = revwalk.push_head() {
                    let mut count = 0;
                    for oid in revwalk {
                        if count >= 5 {
                            break;
                        }
                        if let Ok(oid) = oid {
                            if let Ok(commit) = repo.find_commit(oid) {
                                let summary = commit.summary().unwrap_or("<no summary>");
                                let id = commit.id();
                                result.push_str(&format!("{id:.7} {summary}\n"));
                                count += 1;
                            }
                        }
                    }
                    if count == 0 {
                        result.push_str("<no commits>\n");
                    }
                } else {
                    result.push_str("<no commits>\n");
                }
            }
            Err(_) => {
                result.push_str("<no commits>\n");
            }
        }

        Ok(result)
    }
}

/// Common directory structure functionality for workspaces
pub struct DirectoryStructureUtils;

impl DirectoryStructureUtils {
    /// Get directory structure with limited depth
    pub fn get_directory_structure(
        root_path: &Path,
        max_depth: usize,
    ) -> Result<String, std::io::Error> {
        let mut structure = vec![root_path.display().to_string()];

        // Simple directory traversal (limited depth to avoid huge responses)
        Self::collect_directory_paths(root_path, &mut structure, 0, max_depth)?;

        structure.sort();
        Ok(structure.join("\n"))
    }

    /// Recursively collect directory paths
    fn collect_directory_paths(
        dir: &Path,
        paths: &mut Vec<String>,
        current_depth: usize,
        max_depth: usize,
    ) -> Result<(), std::io::Error> {
        if current_depth >= max_depth {
            return Ok(());
        }

        let entries = std::fs::read_dir(dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Get relative path from the original root directory
            if let Some(rel_path) = path.file_name() {
                let path_str = rel_path.to_string_lossy().to_string();
                if path.is_dir() {
                    paths.push(format!("{path_str}/"));
                    Self::collect_directory_paths(&path, paths, current_depth + 1, max_depth)?;
                } else {
                    paths.push(path_str);
                }
            }
        }

        Ok(())
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
        git2::Repository::discover(path).is_ok()
    }

    /// Read README.md if it exists
    pub fn read_readme(path: &Path) -> Option<String> {
        let readme_path = path.join("README.md");
        std::fs::read_to_string(readme_path).ok()
    }

    /// Read CLAUDE.md if it exists
    pub fn read_claude_md(path: &Path) -> Option<String> {
        let claude_path = path.join("CLAUDE.md");
        std::fs::read_to_string(claude_path).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        git2::Repository::init(temp_dir.path()).unwrap();
        assert!(EnvironmentUtils::is_git_repo(temp_dir.path()));
    }
}
