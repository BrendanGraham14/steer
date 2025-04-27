use anyhow::{Context, Result};
use glob::Pattern;
use std::fs;
use std::path::Path;

/// List files and directories in a given path
pub fn list_directory(dir_path: &str, ignore_patterns: &[String]) -> Result<String> {
    let path = Path::new(dir_path);

    // Check if path exists and is a directory
    if !path.exists() {
        return Err(anyhow::anyhow!("Path does not exist: {}", dir_path));
    }

    if !path.is_dir() {
        return Err(anyhow::anyhow!("Not a directory: {}", dir_path));
    }

    // Compile ignore patterns
    let compiled_patterns: Vec<Pattern> = ignore_patterns
        .iter()
        .filter_map(|pattern| Pattern::new(pattern).ok())
        .collect();

    // Read directory
    let entries = fs::read_dir(path).context(format!("Failed to read directory: {}", dir_path))?;

    // Process entries
    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry_result in entries {
        let entry = entry_result.context("Failed to read directory entry")?;
        let entry_path = entry.path();
        let file_name = entry_path.file_name().unwrap_or_default().to_string_lossy();

        // Skip entries matching ignore patterns
        let should_ignore = compiled_patterns
            .iter()
            .any(|pattern| pattern.matches(&file_name));
        if should_ignore {
            continue;
        }

        // Add to appropriate list
        if entry_path.is_dir() {
            dirs.push(format!("{}/", file_name));
        } else {
            files.push(file_name.to_string());
        }
    }

    // Sort entries
    dirs.sort();
    files.sort();

    // Format output
    let mut output = String::new();

    output.push_str(&format!("Directory contents of {}:\n\n", dir_path));

    if dirs.is_empty() && files.is_empty() {
        output.push_str("Directory is empty.\n");
    } else {
        if !dirs.is_empty() {
            output.push_str("Directories:\n");
            for dir in dirs {
                output.push_str(&format!("- {}\n", dir));
            }
            output.push('\n');
        }

        if !files.is_empty() {
            output.push_str("Files:\n");
            for file in files {
                output.push_str(&format!("- {}\n", file));
            }
        }
    }

    Ok(output)
}
