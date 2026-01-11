use ignore::WalkBuilder;
use std::path::Path;

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
                item_count += 1;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
