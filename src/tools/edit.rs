use anyhow::{Context, Result};
use std::path::Path;
use tokio::fs;

// TODO: Refactor edit_file to use async file I/O (tokio::fs)
// and potentially check cancellation token, especially for large files.
// Currently, fs::read_to_string and fs::write can block.

/// Edit a file by replacing an old string with a new string (async)
pub async fn edit_file(file_path: &str, old_string: &str, new_string: &str) -> Result<String> {
    let path = Path::new(file_path);

    // Handle new file creation
    if old_string.is_empty() {
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

        // Write the new file asynchronously
        fs::write(path, new_string)
            .await
            .context(format!("Failed to create file: {}", file_path))?;

        return Ok(format!("File created: {}", file_path));
    }

    // Check if file exists for editing asynchronously
    if !fs::metadata(path)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        return Err(anyhow::anyhow!(
            "File not found or is not a file: {}",
            file_path
        ));
    }

    // Read the file content asynchronously
    let content = fs::read_to_string(path)
        .await
        .context(format!("Failed to read file: {}", file_path))?;

    // Count occurrences of the old string (synchronous)
    let occurrences = content.matches(old_string).count();

    if occurrences == 0 {
        return Err(anyhow::anyhow!("String not found in file: {}", file_path));
    }

    if occurrences > 1 {
        return Err(anyhow::anyhow!(
            "Found {} occurrences of the string in file: {}. Please provide more context to uniquely identify the instance to replace.",
            occurrences,
            file_path
        ));
    }

    // Replace the string (synchronous)
    let new_content = content.replace(old_string, new_string);

    // Write the updated content asynchronously
    fs::write(path, new_content)
        .await
        .context(format!("Failed to write file: {}", file_path))?;

    Ok(format!("File edited: {}", file_path))
}
