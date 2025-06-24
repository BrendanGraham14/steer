use conductor_macros::tool;
use glob;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use tokio::task;

use crate::{ExecutionContext, ToolError};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GrepParams {
    /// The search pattern (regex or literal string). If invalid regex, searches for literal text
    pub pattern: String,
    /// Optional glob pattern to filter files by name (e.g., "*.rs", "*.{ts,tsx}")
    pub include: Option<String>,
    /// Optional directory to search in (defaults to current working directory)
    pub path: Option<String>,
}

tool! {
    GrepTool {
        params: GrepParams,
        description: r#"Fast content search built on ripgrep for blazing performance at any scale.
- Searches using regular expressions or literal strings
- Supports regex syntax like "log.*Error", "function\\s+\\w+", etc.
- If the pattern isn't valid regex, it automatically searches for the literal text
- Filter files by name pattern with include parameter (e.g., "*.js", "*.{ts,tsx}")
- Automatically respects .gitignore files
- Returns matches as "filepath:line_number: line_content""#,
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

    // Create matcher - try regex first, fall back to literal if it fails
    let matcher = match RegexMatcherBuilder::new()
        .line_terminator(Some(b'\n'))
        .build(pattern)
    {
        Ok(m) => m,
        Err(_) => {
            // Fall back to literal search by escaping the pattern
            let escaped = regex::escape(pattern);
            RegexMatcherBuilder::new()
                .line_terminator(Some(b'\n'))
                .build(&escaped)
                .map_err(|e| format!("Failed to create matcher: {}", e))?
        }
    };

    // Build the searcher with binary detection
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .build();

    // Use ignore crate's WalkBuilder for respecting .gitignore
    let mut walker = WalkBuilder::new(base_path);
    walker.hidden(false); // Include hidden files by default
    walker.git_ignore(true); // Respect .gitignore files
    walker.git_global(true); // Respect global gitignore
    walker.git_exclude(true); // Respect .git/info/exclude

    let include_pattern = include
        .map(|p| glob::Pattern::new(p).map_err(|e| format!("Invalid glob pattern: {}", e)))
        .transpose()?;

    let mut results = Vec::new();

    for result in walker.build() {
        if cancellation_token.is_cancelled() {
            return Err("Search cancelled".to_string());
        }

        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Check include pattern if specified
        if let Some(ref pattern) = include_pattern {
            if !path_matches_glob(path, pattern, base_path) {
                continue;
            }
        }

        // Search the file
        let mut matches_in_file = Vec::new();
        let search_result = searcher.search_path(
            &matcher,
            path,
            UTF8(|line_num, line| {
                // Canonicalize the path for clean output
                let display_path = match path.canonicalize() {
                    Ok(canonical) => canonical.display().to_string(),
                    // If canonicalization fails (e.g., file doesn't exist), fall back to regular display
                    Err(_) => path.display().to_string(),
                };
                matches_in_file.push(format!("{}:{}: {}", display_path, line_num, line));
                Ok(true)
            }),
        );

        if let Err(e) = search_result {
            // Skip files that can't be searched (e.g., binary files)
            if e.kind() == std::io::ErrorKind::InvalidData {
                continue;
            }
        }

        if !matches_in_file.is_empty() {
            results.push((path.to_path_buf(), matches_in_file.join("\n")));
        }
    }

    if results.is_empty() {
        return Ok("No matches found.".to_string());
    }

    // Sort by modification time (newest first)
    let mut results_with_time = Vec::new();
    for (path, matches) in results {
        if cancellation_token.is_cancelled() {
            return Err("Search cancelled".to_string());
        }

        let mtime = fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        results_with_time.push((path, matches, mtime));
    }

    results_with_time.sort_by(|a, b| b.2.cmp(&a.2));

    let output = results_with_time
        .into_iter()
        .map(|(_, matches, _)| matches)
        .collect::<Vec<_>>()
        .join("\n\n");

    Ok(output)
}

fn path_matches_glob(path: &Path, pattern: &glob::Pattern, base_path: &Path) -> bool {
    // Check if the full path matches
    if pattern.matches_path(path) {
        return true;
    }

    // Check if the relative path from base_path matches
    if let Ok(relative_path) = path.strip_prefix(base_path) {
        if pattern.matches_path(relative_path) {
            return true;
        }
    }

    // Also check if just the filename matches (for patterns like "*.rs")
    if let Some(filename) = path.file_name() {
        if pattern.matches(&filename.to_string_lossy()) {
            return true;
        }
    }

    false
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
        fs::write(dir.join("binary.dat"), [0, 159, 146, 150]).unwrap();
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

    #[tokio::test]
    async fn test_grep_literal_fallback() {
        let temp_dir = tempdir().unwrap();

        // Create test files with patterns that would fail as regex
        fs::write(
            temp_dir.path().join("code.rs"),
            "fn main() {\n    format_message(\"hello\");\n    println!(\"world\");\n}",
        )
        .unwrap();

        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: "format_message(".to_string(), // This would fail as regex due to unclosed (
            include: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        // Should find the literal match
        assert!(result.contains("code.rs:2:     format_message(\"hello\");"));
        assert!(!result.contains("println!"));
    }

    #[tokio::test]
    async fn test_grep_relative_path_glob_matching() {
        let temp_dir = tempdir().unwrap();

        // Create nested directory structure similar to conductor project
        fs::create_dir_all(temp_dir.path().join("conductor/src/session")).unwrap();
        fs::create_dir_all(temp_dir.path().join("conductor/src/utils")).unwrap();
        fs::create_dir_all(temp_dir.path().join("other/src")).unwrap();

        // Create test files
        fs::write(
            temp_dir.path().join("conductor/src/session/state.rs"),
            "pub struct SessionConfig {\n    pub field: String,\n}",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("conductor/src/utils/session.rs"),
            "use crate::SessionConfig;\nfn test() -> SessionConfig {\n    SessionConfig { field: \"test\".to_string() }\n}",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("other/src/main.rs"),
            "struct SessionConfig;\nfn main() {}",
        )
        .unwrap();

        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        let params = GrepParams {
            pattern: "SessionConfig \\{".to_string(),
            include: Some("conductor/src/**/*.rs".to_string()),
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        // Should find matches in conductor/src files but not in other/src
        assert!(result.contains("conductor/src/session/state.rs:1: pub struct SessionConfig {"));
        assert!(result.contains(
            "conductor/src/utils/session.rs:3:     SessionConfig { field: \"test\".to_string() }"
        ));
        assert!(!result.contains("other/src/main.rs"));
    }

    #[tokio::test]
    async fn test_grep_complex_relative_patterns() {
        let temp_dir = tempdir().unwrap();

        // Create complex directory structure
        fs::create_dir_all(temp_dir.path().join("src/api/client")).unwrap();
        fs::create_dir_all(temp_dir.path().join("src/tools")).unwrap();
        fs::create_dir_all(temp_dir.path().join("tests/integration")).unwrap();

        // Create test files
        fs::write(
            temp_dir.path().join("src/api/client/mod.rs"),
            "pub mod client;\npub use client::ApiClient;",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("src/tools/grep.rs"),
            "pub struct GrepTool;\nimpl Tool for GrepTool {}",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("tests/integration/api_test.rs"),
            "use crate::api::ApiClient;\n#[test]\nfn test_api() {}",
        )
        .unwrap();

        let context = create_test_context(&temp_dir);

        // Test pattern that should match only src/**/*.rs files
        let tool = GrepTool;
        let params = GrepParams {
            pattern: "pub".to_string(),
            include: Some("src/**/*.rs".to_string()),
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        // Should find matches in src/ but not in tests/
        assert!(result.contains("src/api/client/mod.rs:1: pub mod client;"));
        assert!(result.contains("src/api/client/mod.rs:2: pub use client::ApiClient;"));
        assert!(result.contains("src/tools/grep.rs:1: pub struct GrepTool;"));
        assert!(!result.contains("tests/integration/api_test.rs"));
    }

    #[tokio::test]
    async fn test_grep_canonicalized_paths() {
        let temp_dir = tempdir().unwrap();

        // Create a test file
        fs::write(
            temp_dir.path().join("test.txt"),
            "line one\nfind this line\nline three",
        )
        .unwrap();

        let context = create_test_context(&temp_dir);

        let tool = GrepTool;
        // Use "." as the path to simulate the issue
        let params = GrepParams {
            pattern: "find this".to_string(),
            include: None,
            path: Some(".".to_string()),
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        // The result should contain a canonicalized path without "./"
        assert!(result.contains(":2: find this line"));
        // Ensure no "./" appears in the path
        assert!(!result.contains("./"));

        // The path should be absolute and canonical
        let canonical_path = temp_dir.path().join("test.txt").canonicalize().unwrap();
        assert!(result.contains(&canonical_path.display().to_string()));
    }
}
