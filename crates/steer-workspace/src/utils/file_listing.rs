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
        if let Some(max) = max_results
            && max > 0
            && filtered_files.len() > max
        {
            filtered_files.truncate(max);
        }

        Ok(filtered_files)
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

        // Restore permissions for cleanup
        let mut perms = std::fs::metadata(&restricted_dir).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&restricted_dir, perms).unwrap();
    }
}
