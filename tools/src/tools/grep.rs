use glob;
use ignore::WalkBuilder;
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
- Automatically respects .gitignore files and other git exclusion rules
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

    // Use the ignore crate's WalkBuilder which respects .gitignore files
    let mut walker = WalkBuilder::new(base_path);
    walker.hidden(false); // Include hidden files by default
    walker.git_ignore(true); // Respect .gitignore files
    walker.git_global(true); // Respect global gitignore
    walker.git_exclude(true); // Respect .git/info/exclude

    let walk = walker.build();

    for result in walk {
        if cancellation_token.is_cancelled() {
            return Err("Search cancelled".to_string());
        }

        match result {
            Ok(entry) => {
                let path = entry.path();
                if path.is_file() && is_likely_text_file(path) {
                    // If include pattern is specified, check if file matches
                    if let Some(include_pattern) = include {
                        if path_matches_glob(path, include_pattern)? {
                            files.push(path.to_path_buf());
                        }
                    } else {
                        files.push(path.to_path_buf());
                    }
                }
            }
            Err(_) => {
                // Skip entries that can't be processed
                continue;
            }
        }
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

fn path_matches_glob(path: &Path, pattern: &str) -> Result<bool, String> {
    // Convert the file path to a string for glob matching
    let path_str = path.to_string_lossy();

    // Create a glob pattern
    let glob_pattern = glob::Pattern::new(pattern)
        .map_err(|e| format!("Invalid glob pattern '{}': {}", pattern, e))?;

    // Check if the full path matches
    if glob_pattern.matches(&path_str) {
        return Ok(true);
    }

    // Also check if just the filename matches (for patterns like "*.rs")
    if let Some(filename) = path.file_name() {
        if glob_pattern.matches(&filename.to_string_lossy()) {
            return Ok(true);
        }
    }

    Ok(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionContext, Tool, ToolError};
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    fn create_test_files(dir: &Path) {
        fs::write(dir.join("file1.txt"), "hello world\nfind me here").unwrap();
        fs::write(
            dir.join("file2.log"),
            "another file\nwith some logs\nLOG-123: an error",
        )
        .unwrap();
        fs::create_dir(dir.join("subdir")).unwrap();
        fs::write(
            dir.join("subdir/file3.txt"),
            "nested file\nshould also be found",
        )
        .unwrap();
        // A file with non-utf8 content to test robustness
        fs::write(dir.join("binary.dat"), &[0, 159, 146, 150]).unwrap();
    }

    fn create_test_context(temp_dir: &tempfile::TempDir) -> ExecutionContext {
        ExecutionContext::new("test-call-id".to_string())
            .with_working_directory(temp_dir.path().to_path_buf())
    }

    #[tokio::test]
    async fn test_grep_simple_match() {
        let temp_dir = tempdir().unwrap();
        create_test_files(temp_dir.path());
        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: "find me".to_string(),
            include: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        assert!(result.contains("file1.txt:2: find me here"));
        assert!(!result.contains("file2.log"));
    }

    #[tokio::test]
    async fn test_grep_regex_match() {
        let temp_dir = tempdir().unwrap();
        create_test_files(temp_dir.path());
        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: r"LOG-\d+".to_string(),
            include: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        assert!(result.contains("file2.log:3: LOG-123: an error"));
        assert!(!result.contains("file1.txt"));
    }

    #[tokio::test]
    async fn test_grep_no_matches() {
        let temp_dir = tempdir().unwrap();
        create_test_files(temp_dir.path());
        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: "non-existent pattern".to_string(),
            include: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        assert_eq!(result, "No matches found.");
    }

    #[tokio::test]
    async fn test_grep_with_path() {
        let temp_dir = tempdir().unwrap();
        create_test_files(temp_dir.path());
        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: "nested".to_string(),
            include: None,
            path: Some("subdir".to_string()),
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        assert!(result.contains("subdir/file3.txt:1: nested file"));
        assert!(!result.contains("file1.txt"));
        assert!(!result.contains("file2.log"));
    }

    #[tokio::test]
    async fn test_grep_with_include() {
        let temp_dir = tempdir().unwrap();
        create_test_files(temp_dir.path());
        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: "file".to_string(),
            include: Some("*.log".to_string()),
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        assert!(result.contains("file2.log:1: another file"));
        assert!(!result.contains("file1.txt"));
    }

    #[tokio::test]
    async fn test_grep_non_existent_path() {
        let temp_dir = tempdir().unwrap();
        create_test_files(temp_dir.path());
        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: "any".to_string(),
            include: None,
            path: Some("non-existent-dir".to_string()),
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await;

        assert!(matches!(result, Err(ToolError::Execution { .. })));
        if let Err(ToolError::Execution { message, .. }) = result {
            assert!(message.contains("Path does not exist"));
        }
    }

    #[tokio::test]
    async fn test_grep_cancellation() {
        let temp_dir = tempdir().unwrap();
        create_test_files(temp_dir.path());

        let token = CancellationToken::new();
        token.cancel(); // Cancel immediately

        let context = ExecutionContext::new("test-call-id".to_string())
            .with_working_directory(temp_dir.path().to_path_buf())
            .with_cancellation_token(token);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: "hello".to_string(),
            include: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await;

        assert!(matches!(result, Err(ToolError::Cancelled(_))));
    }

    #[tokio::test]
    async fn test_grep_respects_gitignore() {
        let temp_dir = tempdir().unwrap();

        // Initialize a git repository (required for .gitignore to work)
        fs::create_dir(temp_dir.path().join(".git")).unwrap();

        // Create test files
        fs::write(
            temp_dir.path().join("file1.txt"),
            "hello world\nfind me here",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("ignored.txt"),
            "this should be ignored\nfind me here",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("also_ignored.log"),
            "another ignored file\nfind me here",
        )
        .unwrap();

        // Create .gitignore file
        fs::write(temp_dir.path().join(".gitignore"), "ignored.txt\n*.log").unwrap();

        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: "find me here".to_string(),
            include: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        // Should find the match in file1.txt but not in ignored files
        assert!(result.contains("file1.txt:2: find me here"));
        assert!(!result.contains("ignored.txt"));
        assert!(!result.contains("also_ignored.log"));
    }
}
