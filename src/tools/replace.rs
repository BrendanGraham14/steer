use anyhow::{Context, Result};
use std::path::Path;
use std::fs;

/// Write a file to the filesystem
pub fn replace_file(file_path: &str, content: &str) -> Result<String> {
    let path = Path::new(file_path);
    
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .context(format!("Failed to create directory: {}", parent.display()))?;
        }
    }
    
    // Write the file
    fs::write(path, content)
        .context(format!("Failed to write file: {}", file_path))?;
    
    Ok(format!("File written: {}", file_path))
}