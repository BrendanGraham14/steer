use anyhow::{Context, Result};
use glob::glob;
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolError;
use coder_macros::tool;

// Derive JsonSchema for parameters
#[derive(Deserialize, Debug, JsonSchema)]
struct GrepParams {
    /// The regular expression pattern to search for
    pattern: String,
    /// Optional file pattern to include in the search (e.g., "*.rs")
    include: Option<String>,
    /// Optional directory to search in (defaults to ".")
    path: Option<String>,
}

tool! {
    GrepTool {
        params: GrepParams,
        name: "grep",
        description: r#"- Fast content search tool that works with any codebase size
- Searches file contents using regular expressions
- Supports full regex syntax (eg. \"log._Error\", \"function\\s+\\w+\", etc.)
- Filter files by pattern with the include parameter (eg. \"_.js\", \"_.{ts,tsx}\")
- Returns matching file paths sorted by modification time
- Use this tool when you need to find files containing specific patterns
- When you are doing an open ended search that may require multiple rounds of globbing and grepping, use the Agent tool instead"#,
    }

    // Move the run function definition inside the macro invocation
    async fn run(
        _tool: &GrepTool,
        params: GrepParams,
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError> {
        // Cancellation check
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("GrepTool".to_string()));
            }
        }

        // Call internal synchronous logic
        grep_search_internal(
            &params.pattern,
            params.include.as_deref(),
            params.path.as_deref().unwrap_or("."),
        )
        .map_err(|e| ToolError::execution("GrepTool", e))
    }
}

// Keep the original grep_search logic, but make it internal
// and return anyhow::Result so we can map it to ToolError
fn grep_search_internal(pattern: &str, include: Option<&str>, path: &str) -> Result<String> {
    let base_path = Path::new(path);

    if !base_path.exists() {
        return Err(anyhow::anyhow!("Path does not exist: {}", path));
    }

    let regex = Regex::new(pattern).context(format!("Invalid regex pattern: {}", pattern))?;
    let files = find_files(base_path, include)?;
    let mut results = Vec::new();

    for file_path in files {
        // Use std::fs::File::open which returns std::io::Result
        match fs::File::open(&file_path) {
            Ok(file) => {
                let reader = BufReader::new(file);
                let mut matched = false;
                let mut matches_in_file = Vec::new();

                for (i, line_result) in reader.lines().enumerate() {
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
                        Err(e) => {
                            // Log or report the error reading a line
                            crate::utils::logging::warn(
                                "tools.grep.read_line_error",
                                &format!("Error reading line in {}: {}", file_path.display(), e),
                            );
                        }
                    }
                }

                if matched {
                    results.push(matches_in_file.join("\n"));
                }
            }
            Err(e) => {
                // Log or report the error opening the file
                crate::utils::logging::warn(
                    "tools.grep.open_file_error",
                    &format!("Failed to open file {}: {}", file_path.display(), e),
                );
                // Optionally, continue to next file or return an error
                // For now, we just skip this file.
            }
        }
    }

    if results.is_empty() {
        Ok("No matches found.".to_string())
    } else {
        Ok(results.join("\n\n"))
    }
}

/// Find files to search based on include pattern
fn find_files(base_path: &Path, include: Option<&str>) -> Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();

    if let Some(include_pattern) = include {
        // Use glob to find files based on include pattern
        let glob_pattern = if base_path.to_string_lossy() == "." {
            include_pattern.to_string()
        } else {
            format!("{}/{}", base_path.display(), include_pattern)
        };

        for entry in
            glob(&glob_pattern).context(format!("Invalid include pattern: {}", include_pattern))?
        {
            if let Ok(path) = entry {
                if path.is_file() {
                    files.push(path);
                }
            }
        }
    } else {
        // Recursively find all regular files
        find_files_recursive(base_path, &mut files)?;
    }

    // Sort files by modification time (newest first)
    let mut files_with_time = Vec::new();
    for path in files {
        if let Ok(metadata) = fs::metadata(&path) {
            if let Ok(modified) = metadata.modified() {
                files_with_time.push((path, modified));
            } else {
                files_with_time.push((path, std::time::SystemTime::UNIX_EPOCH));
            }
        } else {
            // If metadata fails, treat it as very old for sorting purposes
            files_with_time.push((path, std::time::SystemTime::UNIX_EPOCH));
        }
    }

    files_with_time.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(files_with_time.into_iter().map(|(path, _)| path).collect())
}

/// Recursively find all regular files in a directory
fn find_files_recursive(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> io::Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                find_files_recursive(&path, files)?;
            } else if path.is_file() {
                // Skip binary files
                if is_likely_text_file(&path)? {
                    files.push(path);
                }
            }
        }
    }
    Ok(())
}

/// Check if a file is likely a text file
fn is_likely_text_file(path: &Path) -> io::Result<bool> {
    // Check extension first
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        let binary_extensions = [
            "exe", "dll", "so", "dylib", "bin", "jpg", "jpeg", "png", "gif", "bmp", "ico", "pdf",
            "zip", "tar", "gz", "7z", "rar", "mp3", "mp4", "avi", "mov", "flv", "wav", "wma",
            "ogg", "class", "pyc", "o", "a", "lib", "obj", "pdb",
        ];

        if binary_extensions.contains(&ext_str.as_str()) {
            return Ok(false);
        }
    }

    // Read first 1KB to check for binary content
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false), // File vanished?
        Err(e) => return Err(e),                                           // Other metadata error
    };
    if metadata.len() == 0 {
        return Ok(true); // Empty file is text
    }

    let mut buffer = vec![0; std::cmp::min(1024, metadata.len() as usize)];
    // Use a block to ensure file is closed promptly
    {
        let mut file = match fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false), // File vanished?
            Err(e) => return Err(e),                                           // Other open error
        };
        use std::io::Read;
        // Handle potential short reads, although read_exact is usually fine here
        if let Err(e) = file.read_exact(&mut buffer) {
            // If the file shrunk between metadata check and read, treat as error or non-text
            // Depending on desired behavior, could return Ok(false) or Err(e)
            return Err(io::Error::new(
                e.kind(),
                format!(
                    "Failed to read first {} bytes from {}: {}",
                    buffer.len(),
                    path.display(),
                    e
                ),
            ));
        }
    }

    // Check for null bytes and other binary indicators
    // Allow UTF-8 BOM (EF BB BF)
    if buffer.starts_with(&[0xEF, 0xBB, 0xBF]) {
        // Check remaining bytes after BOM
        Ok(!buffer[3..].iter().any(|&b| b == 0)
            && buffer[3..].iter().filter(|&&b| b < 9).count() == 0)
    } else {
        // No BOM, check entire buffer
        Ok(!buffer.iter().any(|&b| b == 0) && buffer.iter().filter(|&&b| b < 9).count() == 0)
    }
}

/* Remove old public function if desired
/// Search for files containing a regex pattern
pub fn grep_search(pattern: &str, include: Option<&str>, path: &str) -> Result<String> {
    grep_search_internal(pattern, include, path)
}
*/
