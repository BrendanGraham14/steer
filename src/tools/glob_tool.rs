use anyhow::{Context, Result};
use glob::glob;
use std::fs;
use std::path::Path;

/// Search for files matching a glob pattern
pub fn glob_search(pattern: &str, path: &str) -> Result<String> {
    let base_path = Path::new(path);

    // Ensure the path exists
    if !base_path.exists() {
        return Err(anyhow::anyhow!("Path does not exist: {}", path));
    }

    // Calculate the full pattern
    let full_pattern = if base_path.to_string_lossy() == "." {
        pattern.to_string()
    } else {
        format!("{}/{}", base_path.display(), pattern)
    };

    // Search for matching files
    let paths = glob(&full_pattern)
        .context(format!("Invalid glob pattern: {}", full_pattern))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();

    // Sort paths by modification time (newest first)
    let mut paths_with_time = Vec::new();
    for path in paths {
        if let Ok(metadata) = fs::metadata(&path) {
            if let Ok(modified) = metadata.modified() {
                paths_with_time.push((path, modified));
            } else {
                paths_with_time.push((path, std::time::SystemTime::UNIX_EPOCH));
            }
        }
    }

    paths_with_time.sort_by(|a, b| b.1.cmp(&a.1));

    // Format output
    if paths_with_time.is_empty() {
        Ok("No files found matching pattern.".to_string())
    } else {
        let results = paths_with_time
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(results)
    }
}
