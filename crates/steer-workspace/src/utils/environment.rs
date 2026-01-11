use std::path::Path;

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
    use tempfile::tempdir;

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
}
