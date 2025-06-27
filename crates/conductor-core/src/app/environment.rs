use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Information about the environment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub working_directory: PathBuf,
    pub is_git_repo: bool,
    pub platform: String,
    pub date: String,
    pub directory_structure: String,
    pub git_status: Option<String>,
    pub readme_content: Option<String>,
    pub claude_md_content: Option<String>,
}

impl EnvironmentInfo {
    /// Collect information about the environment
    pub fn collect() -> Result<Self> {
        let working_directory =
            std::env::current_dir().context("Failed to get current directory")?;
        Self::collect_for_path(&working_directory)
    }

    /// Collect information about the environment for a specific path
    pub fn collect_for_path(workspace_path: &Path) -> Result<Self> {
        let working_directory = workspace_path.to_path_buf();

        // Check if we're in a git repo
        let is_git_repo =
            workspace_path.join(".git").exists() || Self::is_git_repo(workspace_path)?;

        // Get platform information
        let platform = Self::get_platform();

        // Get current date
        let date = Self::get_date();

        // Get directory structure
        let directory_structure = Self::get_directory_structure(workspace_path)?;

        // Get git status if in a repo
        let git_status = if is_git_repo {
            Some(Self::get_git_status(workspace_path)?)
        } else {
            None
        };

        // Check for README.md
        let readme_content = Self::read_file_if_exists(workspace_path, "README.md");

        // Check for CLAUDE.md
        let claude_md_content = Self::read_file_if_exists(workspace_path, "CLAUDE.md");

        Ok(Self {
            working_directory,
            is_git_repo,
            platform,
            date,
            directory_structure,
            git_status,
            readme_content,
            claude_md_content,
        })
    }

    /// Check if the specified directory is in a git repo
    fn is_git_repo(path: &Path) -> Result<bool> {
        let output = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(path)
            .output();

        match output {
            Ok(output) if output.status.success() => Ok(true),
            _ => Ok(false),
        }
    }

    /// Get the platform information
    fn get_platform() -> String {
        let os = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "unknown"
        };

        os.to_string()
    }

    /// Get the current date
    fn get_date() -> String {
        use chrono::Local;
        Local::now().format("%Y-%m-%d").to_string()
    }

    fn get_directory_structure(dir: &Path) -> Result<String> {
        // Start with the base directory path
        let mut all_paths = vec![dir.display().to_string()];

        // Build globset, handling potential errors
        let gitignore_globset = Self::build_gitignore_globset(dir)?;

        // Walk the directory recursively and collect all relative file/dir paths
        let mut relative_paths = Vec::new();
        Self::collect_paths(dir, dir, &gitignore_globset, &mut relative_paths)?;

        // Sort relative paths for consistent output
        relative_paths.sort();

        // Add sorted relative paths to the main list
        all_paths.extend(relative_paths);

        // Join all paths with newline and add a trailing newline
        let result = all_paths.join("\n");
        Ok(format!("{}\n", result)) // Ensure trailing newline
    }

    /// Recursively collect paths in the directory, filtering by gitignore
    fn collect_paths(
        base_dir: &Path,
        current_dir: &Path,
        gitignore_globset: &Option<GlobSet>,
        paths: &mut Vec<String>, // Renamed to relative_paths in caller, but param name is fine
    ) -> Result<()> {
        let entries = fs::read_dir(current_dir).context("Failed to read directory")?;

        for entry in entries {
            let entry = entry.context("Failed to read directory entry")?;
            let path = entry.path();
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();

            // Skip hidden files and directories
            if file_name.starts_with('.') {
                continue;
            }

            // Skip files/dirs that match gitignore patterns
            if Self::is_ignored(&path, base_dir, gitignore_globset) {
                continue;
            }

            // Add path relative to base directory
            if let Ok(rel_path) = path.strip_prefix(base_dir) {
                let path_str = rel_path.to_string_lossy().to_string();
                if path.is_dir() {
                    paths.push(format!("{}/", path_str));

                    // Recursively process subdirectories
                    // Pass the same 'paths' vec down
                    Self::collect_paths(base_dir, &path, gitignore_globset, paths)?;
                } else {
                    paths.push(path_str);
                }
            }
        }

        Ok(())
    }

    /// Get git status information for a specific path
    fn get_git_status(path: &Path) -> Result<String> {
        // TODO: We should use the git crate instead.
        let mut result = String::new();

        // Get current branch
        let branch_output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(path)
            .output()
            .context("Failed to run git branch")?;

        let branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();
        result.push_str(&format!("Current branch: {}\n\n", branch));

        // Get status
        let status_output = Command::new("git")
            .args(["status", "--short"])
            .current_dir(path)
            .output()
            .context("Failed to run git status")?;

        let status = String::from_utf8_lossy(&status_output.stdout)
            .trim()
            .to_string();
        result.push_str("Status:\n");
        if status.is_empty() {
            result.push_str("Working tree clean\n");
        } else {
            result.push_str(&status);
            result.push('\n');
        }

        // Get recent commits
        let log_output = Command::new("git")
            .args(["log", "--oneline", "-n", "5"])
            .current_dir(path)
            .output()
            .context("Failed to run git log")?;

        let log = String::from_utf8_lossy(&log_output.stdout)
            .trim()
            .to_string();
        result.push_str("\nRecent commits:\n");
        result.push_str(&log);

        Ok(result)
    }

    /// Read a file if it exists at a specific path
    fn read_file_if_exists(path: &Path, filename: &str) -> Option<String> {
        let file_path = path.join(filename);
        match fs::read_to_string(file_path) {
            Ok(content) => Some(content),
            Err(_) => None,
        }
    }

    /// Read gitignore patterns from .gitignore file and build a GlobSet
    /// Returns Ok(None) if .gitignore doesn't exist or is empty.
    /// Returns Err if .gitignore exists but cannot be read, or contains invalid patterns.
    fn build_gitignore_globset(dir: &Path) -> Result<Option<GlobSet>> {
        let gitignore_path = dir.join(".gitignore");

        if !gitignore_path.exists() {
            return Ok(None); // No gitignore file is not an error
        }

        // If the file exists but cannot be read, return an error
        let content = fs::read_to_string(&gitignore_path).context(format!(
            "Failed to read .gitignore file at {:?}",
            gitignore_path
        ))?;

        let mut builder = GlobSetBuilder::new();
        #[allow(unused_assignments)]
        let mut has_valid_patterns = false;

        for line in content.lines() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Handle negation (not implemented here, but could be with two separate GlobSets)
            if line.starts_with('!') {
                continue;
            }

            // Handle directory-specific patterns (ending with slash)
            let mut pattern = line.to_string();

            // Convert pattern to work with globset
            // If pattern doesn't start with *, make it match anywhere in path
            if !pattern.starts_with('*') && !pattern.starts_with('/') {
                pattern = format!("**/{}", pattern);
            }

            // If pattern starts with /, remove it (globset assumes patterns are relative)
            if pattern.starts_with('/') {
                pattern = pattern[1..].to_string();
            }

            // Add a trailing ** for directory patterns
            if pattern.ends_with('/') {
                // Remove trailing slash for matching the directory itself
                let dir_pattern = pattern.trim_end_matches('/').to_string();
                match Glob::new(&dir_pattern) {
                    Ok(glob) => {
                        builder.add(glob);
                    }
                    Err(e) => {
                        // Return error for invalid patterns
                        return Err(anyhow::anyhow!(
                            "Invalid glob pattern '{}' in .gitignore: {}",
                            dir_pattern,
                            e
                        ));
                    }
                }
                // Add pattern with ** to match contents
                pattern = format!("{}**", pattern);
            }

            match Glob::new(&pattern) {
                Ok(glob) => {
                    builder.add(glob);
                    has_valid_patterns = true;
                }
                Err(e) => {
                    // Return error for invalid patterns
                    return Err(anyhow::anyhow!(
                        "Invalid glob pattern '{}' in .gitignore: {}",
                        pattern,
                        e
                    ));
                }
            }
        }

        // Only build and return a GlobSet if there were valid patterns
        if !has_valid_patterns {
            return Ok(None); // Empty or comment-only gitignore is not an error
        }

        match builder.build() {
            Ok(globset) => Ok(Some(globset)),
            Err(e) => Err(anyhow::anyhow!(
                "Error building globset from .gitignore: {}",
                e
            )),
        }
    }

    /// Check if a path should be ignored based on gitignore patterns
    fn is_ignored(path: &Path, base_dir: &Path, globset: &Option<GlobSet>) -> bool {
        if let Some(globset) = globset {
            // Get relative path from base directory
            if let Ok(rel_path) = path.strip_prefix(base_dir) {
                // Convert to string for matching
                let path_str = rel_path.to_string_lossy().to_string();

                return globset.is_match(path_str);
            }
        }

        false
    }

    /// Format environment information as context for the model
    pub fn as_context(&self) -> String {
        let mut context = String::new();

        context.push_str(&format!(
            r#"Here is useful information about the environment you are running in:
<env>
Working directory: {}
Is directory a git repo: {}
Platform: {}
Today's date: {}
</env>

<file_structure>
Below is a snapshot of this project's file structure at the start of the conversation. The file structure may be filtered to omit `.gitignore`ed patterns. This snapshot will NOT update during the conversation.

{}
</file_structure>
"#,
            self.working_directory.display(),
            self.is_git_repo,
            self.platform,
            self.date,
            self.directory_structure
        ));

        if let Some(git_status) = &self.git_status {
            context.push_str(&format!(r#"<git_status>
This is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.

{}
</git_status>
"#,
                git_status));
        }

        if let Some(readme) = &self.readme_content {
            context.push_str(&format!(r#"<file name="README.md">
This is the README.md file at the start of the conversation. Note that this README is a snapshot in time, and will not update during the conversation.

{}
</file>
"#,
                readme
            ));
        }

        if let Some(claude_md) = &self.claude_md_content {
            context.push_str(&format!(r#"<file name="CLAUDE.md">
This is the CLAUDE.md file at the start of the conversation. Note that this CLAUDE is a snapshot in time, and will not update during the conversation.

{}
</file>
"#,
                claude_md
            ));
        }

        context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_get_directory_structure() -> Result<()> {
        // Create a temporary directory
        let temp_dir = tempdir()?;
        let temp_path = temp_dir.path();

        // Create nested directories and files
        fs::create_dir(temp_path.join("dir1"))?;
        fs::create_dir(temp_path.join("dir1/subdir"))?;
        fs::create_dir(temp_path.join("dir2"))?;
        fs::create_dir(temp_path.join("ignored_dir"))?; // Should be ignored

        let mut file1 = File::create(temp_path.join("file1.txt"))?;
        file1.write_all(b"content1")?;

        let mut file2 = File::create(temp_path.join("dir1/file2.txt"))?;
        file2.write_all(b"content2")?;

        let mut file3 = File::create(temp_path.join("dir1/subdir/file3.rs"))?;
        file3.write_all(b"fn main() {}")?;

        let mut file4 = File::create(temp_path.join("dir2/file4.md"))?;
        file4.write_all(b"# Title")?;

        let mut ignored_file1 = File::create(temp_path.join("ignored_file.log"))?; // Should be ignored
        ignored_file1.write_all(b"log content")?;

        let mut ignored_file2 = File::create(temp_path.join("ignored_dir/some_file.txt"))?; // Should be ignored
        ignored_file2.write_all(b"nested ignored")?;

        let mut ignored_file3 = File::create(temp_path.join("dir1/subdir/deeply_ignored.tmp"))?; // Should be ignored by **
        ignored_file3.write_all(b"temp content")?;

        // Create .gitignore file
        let mut gitignore = File::create(temp_path.join(".gitignore"))?;
        gitignore.write_all(b"*.log\nignored_dir/\n**/*.tmp\n")?;

        // Expected structure (sorted alphabetically, excluding ignored files/dirs)dispa
        // Note: deeply_ignored.tmp should be excluded due to **/*.tmp
        // Note: ignored_dir and its contents should be excluded
        // Note: ignored_file.log should be excluded
        let mut expected_paths = vec![
            temp_path.display().to_string(),
            "dir1/".to_string(),
            "dir1/file2.txt".to_string(),
            "dir1/subdir/".to_string(),
            "dir1/subdir/file3.rs".to_string(),
            "dir2/".to_string(),
            "dir2/file4.md".to_string(),
            "file1.txt".to_string(),
        ];
        expected_paths.sort();

        // Get the directory structure
        let structure = EnvironmentInfo::get_directory_structure(temp_path)?;

        // Assert that the structure matches the expected output
        // Split the actual output into lines, filter empty lines, and sort for reliable comparison
        let mut actual_lines: Vec<String> = structure
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.to_string())
            .collect();
        actual_lines.sort();

        assert_eq!(actual_lines, expected_paths);

        Ok(())
    }
}
