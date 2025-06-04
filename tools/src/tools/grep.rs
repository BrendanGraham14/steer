use macros::tool;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use tokio::task;

use crate::{ExecutionContext, ToolError};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GrepParams {
    /// The regular expression pattern to search for
    pub pattern: String,
    /// Optional file pattern to include in the search (e.g., "*.rs")
    pub include: Option<String>,
    /// Optional directory to search in (defaults to current working directory)
    pub path: Option<String>,
}

tool! {
    GrepTool {
        params: GrepParams,
        description: r#"Fast content search tool that works with any codebase size.
- Searches file contents using regular expressions
- Supports full regex syntax (eg. "log.*Error", "function\\s+\\w+", etc.)
- Filter files by pattern with the include parameter (eg. "*.js", "*.{ts,tsx}")
- Returns matching file paths sorted by modification time"#,
        name: "grep",
        require_approval: false
    }

    async fn run(
        _tool: &GrepTool,
        params: GrepParams,
        context: &ExecutionContext,
    ) -> Result<String, ToolError> {
        if context.is_cancelled() {
            return Err(ToolError::Cancelled(GREP_TOOL_NAME.to_string()));
        }

        let search_path = params.path.as_deref().unwrap_or(".");
        let base_path = if Path::new(search_path).is_absolute() {
            Path::new(search_path).to_path_buf()
        } else {
            context.working_directory.join(search_path)
        };

        // Run the blocking search operation in a separate task
        let pattern = params.pattern.clone();
        let include = params.include.clone();
        let cancellation_token = context.cancellation_token.clone();

        let result = task::spawn_blocking(move || {
            grep_search_internal(&pattern, include.as_deref(), &base_path, &cancellation_token)
        }).await;

        match result {
            Ok(search_result) => search_result.map_err(|e| ToolError::execution(GREP_TOOL_NAME, e.to_string())),
            Err(e) => Err(ToolError::execution(GREP_TOOL_NAME, format!("Task join error: {}", e))),
        }
    }
}

fn grep_search_internal(
    pattern: &str,
    include: Option<&str>,
    base_path: &Path,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    if !base_path.exists() {
        return Err(format!("Path does not exist: {}", base_path.display()));
    }

    let regex =
        Regex::new(pattern).map_err(|e| format!("Invalid regex pattern '{}': {}", pattern, e))?;

    let files = find_files(base_path, include, cancellation_token)?;
    let mut results = Vec::new();

    for file_path in files {
        if cancellation_token.is_cancelled() {
            return Err("Search cancelled".to_string());
        }

        match fs::File::open(&file_path) {
            Ok(file) => {
                let reader = BufReader::new(file);
                let mut matched = false;
                let mut matches_in_file = Vec::new();

                for (i, line_result) in reader.lines().enumerate() {
                    if cancellation_token.is_cancelled() {
                        return Err("Search cancelled".to_string());
                    }

                    match line_result {
                        Ok(line) => {
                            if regex.is_match(&line) {
                                matched = true;
                                matches_in_file.push(format!(
                                    "{}:{}: {}",
                                    file_path.display(),
                                    i + 1,
                                    line
                                ));
                            }
                        }
                        Err(_) => {
                            // Skip lines that can't be read (e.g., binary content)
                            continue;
                        }
                    }
                }

                if matched {
                    results.push(matches_in_file.join("\n"));
                }
            }
            Err(_) => {
                // Skip files that can't be opened
                continue;
            }
        }
    }

    if results.is_empty() {
        Ok("No matches found.".to_string())
    } else {
        Ok(results.join("\n\n"))
    }
}

fn find_files(
    base_path: &Path,
    include: Option<&str>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<Vec<std::path::PathBuf>, String> {
    let mut files = Vec::new();

    if let Some(include_pattern) = include {
        // Use glob to find files based on include pattern
        let glob_pattern = if base_path.to_string_lossy() == "." {
            include_pattern.to_string()
        } else {
            format!("{}/{}", base_path.display(), include_pattern)
        };

        match glob::glob(&glob_pattern) {
            Ok(paths) => {
                for entry in paths {
                    if cancellation_token.is_cancelled() {
                        return Err("Search cancelled".to_string());
                    }

                    if let Ok(path) = entry {
                        if path.is_file() {
                            files.push(path);
                        }
                    }
                }
            }
            Err(e) => {
                return Err(format!(
                    "Invalid include pattern '{}': {}",
                    include_pattern, e
                ));
            }
        }
    } else {
        // Recursively find all regular files
        find_files_recursive(base_path, &mut files, cancellation_token)?;
    }

    // Sort files by modification time (newest first)
    let mut files_with_time = Vec::new();
    for path in files {
        if cancellation_token.is_cancelled() {
            return Err("Search cancelled".to_string());
        }

        if let Ok(metadata) = fs::metadata(&path) {
            if let Ok(modified) = metadata.modified() {
                files_with_time.push((path, modified));
            } else {
                files_with_time.push((path, std::time::SystemTime::UNIX_EPOCH));
            }
        } else {
            files_with_time.push((path, std::time::SystemTime::UNIX_EPOCH));
        }
    }

    files_with_time.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(files_with_time.into_iter().map(|(path, _)| path).collect())
}

fn find_files_recursive(
    dir: &Path,
    files: &mut Vec<std::path::PathBuf>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    if cancellation_token.is_cancelled() {
        return Err("Search cancelled".to_string());
    }

    if dir.is_dir() {
        match fs::read_dir(dir) {
            Ok(entries) => {
                for entry in entries {
                    if cancellation_token.is_cancelled() {
                        return Err("Search cancelled".to_string());
                    }

                    if let Ok(entry) = entry {
                        let path = entry.path();

                        if path.is_dir() {
                            find_files_recursive(&path, files, cancellation_token)?;
                        } else if path.is_file() {
                            // Skip binary files
                            if is_likely_text_file(&path) {
                                files.push(path);
                            }
                        }
                    }
                }
            }
            Err(_) => {
                // Skip directories that can't be read
                return Ok(());
            }
        }
    }
    Ok(())
}

fn is_likely_text_file(path: &Path) -> bool {
    // Check extension first
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        let binary_extensions = [
            "exe", "dll", "so", "dylib", "bin", "jpg", "jpeg", "png", "gif", "bmp", "ico", "pdf",
            "zip", "tar", "gz", "7z", "rar", "mp3", "mp4", "avi", "mov", "flv", "wav", "wma",
            "ogg", "class", "pyc", "o", "a", "lib", "obj", "pdb",
        ];

        if binary_extensions.contains(&ext_str.as_str()) {
            return false;
        }
    }

    // Read first 1KB to check for binary content
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };

    if metadata.len() == 0 {
        return true; // Empty file is text
    }

    let buffer_size = std::cmp::min(1024, metadata.len() as usize);
    let mut buffer = vec![0; buffer_size];

    match fs::File::open(path) {
        Ok(mut file) => {
            use std::io::Read;
            if file.read_exact(&mut buffer).is_err() {
                return false;
            }
        }
        Err(_) => return false,
    }

    // Check for null bytes and other binary indicators
    if buffer.starts_with(&[0xEF, 0xBB, 0xBF]) {
        // UTF-8 BOM, check remaining bytes
        !buffer[3..].iter().any(|&b| b == 0) && buffer[3..].iter().filter(|&&b| b < 9).count() == 0
    } else {
        // No BOM, check entire buffer
        !buffer.iter().any(|&b| b == 0) && buffer.iter().filter(|&&b| b < 9).count() == 0
    }
}
