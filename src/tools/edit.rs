use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::fs;

/// Edit a file by replacing an old string with a new string
pub fn edit_file(file_path: &str, old_string: &str, new_string: &str) -> Result<String> {
    let path = Path::new(file_path);
    
    // Handle new file creation
    if old_string.is_empty() {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .context(format!("Failed to create directory: {}", parent.display()))?;
            }
        }
        
        // Write the new file
        fs::write(path, new_string)
            .context(format!("Failed to create file: {}", file_path))?;
        
        return Ok(format!("File created: {}", file_path));
    }
    
    // Check if file exists for editing
    if !path.exists() {
        return Err(anyhow::anyhow!("File not found: {}", file_path));
    }
    
    // Read the file content
    let content = fs::read_to_string(path)
        .context(format!("Failed to read file: {}", file_path))?;
    
    // Count occurrences of the old string
    let occurrences = content.matches(old_string).count();
    
    if occurrences == 0 {
        return Err(anyhow::anyhow!("String not found in file: {}", file_path));
    }
    
    if occurrences > 1 {
        return Err(anyhow::anyhow!("Found {} occurrences of the string in file: {}. Please provide more context to uniquely identify the instance to replace.", occurrences, file_path));
    }
    
    // Replace the string
    let new_content = content.replace(old_string, new_string);
    
    // Write the updated content
    fs::write(path, new_content)
        .context(format!("Failed to write file: {}", file_path))?;
    
    Ok(format!("File edited: {}", file_path))
}