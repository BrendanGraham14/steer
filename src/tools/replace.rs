use anyhow::{Context, Result};
use std::path::Path;
use tokio::fs;

// TODO: Refactor replace_file to use async file I/O (tokio::fs)
// and potentially check cancellation token, especially for large files.
// Currently, fs::write can block.

/// Write a file to the filesystem asynchronously
pub async fn replace_file(file_path: &str, content: &str) -> Result<String> {
    let path = Path::new(file_path);

    // Ensure parent directory exists asynchronously
    if let Some(parent) = path.parent() {
        if !fs::metadata(parent)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            fs::create_dir_all(parent)
                .await
                .context(format!("Failed to create directory: {}", parent.display()))?;
        }
    }

    // Write the file asynchronously
    fs::write(path, content)
        .await
        .context(format!("Failed to write file: {}", file_path))?;

    Ok(format!("File written: {}", file_path))
}
