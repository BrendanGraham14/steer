pub mod session;
pub mod tracing;

use std::path::Path;

/// Returns true if the path exists and is a git repository
pub fn is_git_repo(path: &Path) -> bool {
    let git_dir = path.join(".git");
    git_dir.exists() && git_dir.is_dir()
}

/// Returns the platform information as a string
pub fn get_platform() -> String {
    #[cfg(target_os = "linux")]
    return "linux".to_string();

    #[cfg(target_os = "macos")]
    return "macos".to_string();

    #[cfg(target_os = "windows")]
    return "windows".to_string();

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return "unknown".to_string();
}

/// Escapes special characters in a string for use in a regex
pub fn escape_regex(s: &str) -> String {
    let special_chars = r"[\^$.|?*+(){}";
    let mut result = String::with_capacity(s.len() * 2);

    for c in s.chars() {
        if special_chars.contains(c) {
            result.push('\\');
        }
        result.push(c);
    }

    result
}
