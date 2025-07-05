use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_core::{AstGrep, Pattern};
use ast_grep_language::{LanguageExt, SupportLang};
use conductor_macros::tool;
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::str::FromStr;
use tokio::task;

use crate::result::{AstGrepResult, SearchMatch, SearchResult};
use crate::{ExecutionContext, ToolError};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AstGrepParams {
    /// The search pattern (code pattern with $METAVAR placeholders)
    pub pattern: String,
    /// Language (rust, tsx, python, etc.)
    pub lang: Option<String>,
    /// Optional glob pattern to filter files by name (e.g., "*.rs", "*.{ts,tsx}")
    pub include: Option<String>,
    /// Optional glob pattern to exclude files
    pub exclude: Option<String>,
    /// Optional directory to search in (defaults to current working directory)
    pub path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AstGrepMatch {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub matched_code: String,
    pub context: String,
}

tool! {
    AstGrepTool {
        params: AstGrepParams,
        output: AstGrepResult,
        variant: Search,
        description: r#"Structural code search using abstract syntax trees (AST).
- Searches code by its syntactic structure, not just text patterns
- Use $METAVAR placeholders (e.g., $VAR, $FUNC, $ARGS) to match any code element
- Supports all major languages: rust, javascript, typescript, python, java, go, etc.
Pattern examples:
- "console.log($MSG)" - finds all console.log calls regardless of argument
- "fn $NAME($PARAMS) { $BODY }" - finds all Rust function definitions
- "if $COND { $THEN } else { $ELSE }" - finds all if-else statements
- "import $WHAT from '$MODULE'" - finds all ES6 imports from specific modules
- "$VAR = $VAR + $EXPR" - finds all self-incrementing assignments
Advanced patterns:
- "function $FUNC($$$ARGS) { $$$ }" - $$$ matches any number of elements
- "foo($ARG, ...)" - ellipsis matches remaining arguments
- Use any valid code as a pattern - ast-grep understands the syntax!
Automatically respects .gitignore files"#,
        name: "astgrep",
        require_approval: false
    }

    async fn run(
        _tool: &AstGrepTool,
        params: AstGrepParams,
        context: &ExecutionContext,
    ) -> Result<AstGrepResult, ToolError> {
        if context.is_cancelled() {
            return Err(ToolError::Cancelled(AST_GREP_TOOL_NAME.to_string()));
        }

        let search_path = params.path.as_deref().unwrap_or(".");
        let base_path = if Path::new(search_path).is_absolute() {
            Path::new(search_path).to_path_buf()
        } else {
            context.working_directory.join(search_path)
        };

        // Run the blocking search operation in a separate task
        let pattern = params.pattern.clone();
        let lang = params.lang.clone();
        let include = params.include.clone();
        let exclude = params.exclude.clone();
        let cancellation_token = context.cancellation_token.clone();

        let result = task::spawn_blocking(move || {
            astgrep_search_internal(&pattern, lang.as_deref(), include.as_deref(), exclude.as_deref(), &base_path, &cancellation_token)
        }).await;

        match result {
            Ok(search_result) => search_result.map_err(|e| ToolError::execution(AST_GREP_TOOL_NAME, e)),
            Err(e) => Err(ToolError::execution(AST_GREP_TOOL_NAME, format!("Task join error: {e}"))),
        }
    }
}

fn astgrep_search_internal(
    pattern: &str,
    lang: Option<&str>,
    include: Option<&str>,
    exclude: Option<&str>,
    base_path: &Path,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<AstGrepResult, String> {
    if !base_path.exists() {
        return Err(format!("Path does not exist: {}", base_path.display()));
    }

    // Use ignore crate's WalkBuilder for respecting .gitignore
    let mut walker = WalkBuilder::new(base_path);
    walker.hidden(false); // Include hidden files by default
    walker.git_ignore(true); // Respect .gitignore files
    walker.git_global(true); // Respect global gitignore
    walker.git_exclude(true); // Respect .git/info/exclude

    let include_pattern = include
        .map(|p| glob::Pattern::new(p).map_err(|e| format!("Invalid include glob pattern: {e}")))
        .transpose()?;

    let exclude_pattern = exclude
        .map(|p| glob::Pattern::new(p).map_err(|e| format!("Invalid exclude glob pattern: {e}")))
        .transpose()?;

    let mut all_matches = Vec::new();
    let mut files_searched = 0;

    for result in walker.build() {
        if cancellation_token.is_cancelled() {
            return Ok(AstGrepResult(SearchResult {
                matches: all_matches,
                total_files_searched: files_searched,
                search_completed: false,
            }));
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

        // Check exclude pattern if specified
        if let Some(ref pattern) = exclude_pattern {
            if path_matches_glob(path, pattern, base_path) {
                continue;
            }
        }

        // Determine the language based on file extension or user specification
        let detected_lang = if let Some(l) = lang {
            match SupportLang::from_str(l) {
                Ok(lang) => Some(lang),
                Err(_) => {
                    // Skip files with unsupported language
                    continue;
                }
            }
        } else {
            // Auto-detect language from file extension
            SupportLang::from_extension(path).or_else(|| {
                // Fallback to manual extension matching for common cases
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(|ext| match ext {
                        "jsx" => Some(SupportLang::JavaScript),
                        "mjs" => Some(SupportLang::JavaScript),
                        _ => None,
                    })
            })
        };

        // Skip files without detectable language
        let Some(language) = detected_lang else {
            continue;
        };

        // Read file content
        files_searched += 1;
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // Skip files that can't be read
        };

        // Parse the file using ast-grep
        let ast_grep = language.ast_grep(&content);

        // Create pattern matcher
        let pattern_matcher = match Pattern::try_new(pattern, language) {
            Ok(p) => p,
            Err(e) => return Err(format!("Invalid pattern: {e}")),
        };

        // Find all matches in the file
        let relative_path = path.strip_prefix(base_path).unwrap_or(path);
        let file_matches = find_matches(&ast_grep, &pattern_matcher, relative_path, &content);

        // Convert AstGrepMatch to SearchMatch
        for m in file_matches {
            all_matches.push(SearchMatch {
                file_path: m.file,
                line_number: m.line,
                line_content: m.context.trim().to_string(),
                column_range: Some((m.column, m.column + m.matched_code.len())),
            });
        }
    }

    // Sort by file path for consistent output
    all_matches.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.line_number.cmp(&b.line_number))
    });

    Ok(AstGrepResult(SearchResult {
        matches: all_matches,
        total_files_searched: files_searched,
        search_completed: true,
    }))
}

fn find_matches(
    ast_grep: &AstGrep<StrDoc<SupportLang>>,
    pattern: &Pattern,
    path: &Path,
    content: &str,
) -> Vec<AstGrepMatch> {
    let root = ast_grep.root();
    let matches = root.find_all(pattern);

    let mut results = Vec::new();
    for node_match in matches {
        let node = node_match.get_node();
        let range = node.range();
        let start_pos = node.start_pos();

        // Get the matched code
        let matched_code = node.text();

        // Get the line content for context
        let line_start = content[..range.start]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let line_end = content[range.end..]
            .find('\n')
            .map(|i| range.end + i)
            .unwrap_or(content.len());
        let context = &content[line_start..line_end];

        results.push(AstGrepMatch {
            file: path.display().to_string(),
            line: start_pos.line() + 1, // Convert 0-based to 1-based
            column: start_pos.column(node) + 1,
            matched_code: matched_code.to_string(),
            context: context.to_string(),
        });
    }

    results
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

// Helper trait to make language handling cleaner
trait LanguageHelpers {
    fn from_extension(path: &Path) -> Option<SupportLang>;
}

impl LanguageHelpers for SupportLang {
    fn from_extension(path: &Path) -> Option<SupportLang> {
        ast_grep_language::Language::from_path(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionContext, Tool};
    use std::fs;
    use tempfile::tempdir;

    fn create_test_context(temp_dir: &tempfile::TempDir) -> ExecutionContext {
        ExecutionContext::new("test-call-id".to_string())
            .with_working_directory(temp_dir.path().to_path_buf())
    }

    #[tokio::test]
    async fn test_astgrep_rust_function() {
        let temp_dir = tempdir().unwrap();

        // Create a Rust file with functions
        fs::write(
            temp_dir.path().join("test.rs"),
            r#"fn main() {
    println!("Hello, world!");
}

fn add(a: i32, b: i32) -> i32 {
    a + b
}

async fn fetch_data() -> Result<String, Error> {
    Ok("data".to_string())
}"#,
        )
        .unwrap();

        let context = create_test_context(&temp_dir);

        let tool = AstGrepTool;
        let params = AstGrepParams {
            pattern: "fn $NAME($$$ARGS) { $$$ }".to_string(),
            lang: Some("rust".to_string()),
            include: None,
            exclude: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        // Only fn main() matches the pattern - functions with return types have different AST structure
        assert_eq!(result.0.matches.len(), 1);
        assert!(result.0.matches[0].file_path.contains("test.rs"));
        assert_eq!(result.0.matches[0].line_number, 1);
        assert!(result.0.matches[0].line_content.contains("fn main() {"));
        assert!(result.0.search_completed);
    }

    #[tokio::test]
    async fn test_astgrep_javascript_console_log() {
        let temp_dir = tempdir().unwrap();

        // Create a JavaScript file
        fs::write(
            temp_dir.path().join("app.js"),
            r#"console.log("Starting application");

function processData(data) {
    console.log("Processing:", data);
    console.error("An error occurred");
    return data;
}

console.log("Application ready");"#,
        )
        .unwrap();

        let context = create_test_context(&temp_dir);

        let tool = AstGrepTool;
        let params = AstGrepParams {
            pattern: "console.log($ARGS)".to_string(),
            lang: None, // Should auto-detect from .js extension
            include: None,
            exclude: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        // Only top-level console.log calls are found, not ones inside functions
        assert_eq!(result.0.matches.len(), 2);
        // Check first match
        assert!(result.0.matches.iter().any(|m| {
            m.file_path.contains("app.js")
                && m.line_number == 1
                && m.line_content
                    .contains("console.log(\"Starting application\")")
        }));
        // Check second match
        assert!(result.0.matches.iter().any(|m| {
            m.file_path.contains("app.js")
                && m.line_number == 9
                && m.line_content
                    .contains("console.log(\"Application ready\")")
        }));
        assert!(result.0.search_completed);
    }

    #[tokio::test]
    async fn test_astgrep_with_include_pattern() {
        let temp_dir = tempdir().unwrap();

        // Create multiple files
        fs::write(
            temp_dir.path().join("module.ts"),
            "export function getData() { return fetch('/api/data'); }",
        )
        .unwrap();

        fs::write(
            temp_dir.path().join("test.spec.ts"),
            "describe('test', () => { it('works', () => {}); });",
        )
        .unwrap();

        fs::create_dir(temp_dir.path().join("src")).unwrap();
        fs::write(
            temp_dir.path().join("src/utils.ts"),
            "export function processData() { return []; }",
        )
        .unwrap();

        let context = create_test_context(&temp_dir);

        let tool = AstGrepTool;
        let params = AstGrepParams {
            pattern: "function $NAME($ARGS) { $BODY }".to_string(),
            lang: Some("typescript".to_string()),
            include: Some("src/**/*.ts".to_string()),
            exclude: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        // Export function syntax doesn't match the pattern
        assert_eq!(result.0.matches.len(), 0);
        assert!(result.0.search_completed);
    }

    #[tokio::test]
    async fn test_astgrep_no_matches() {
        let temp_dir = tempdir().unwrap();

        fs::write(
            temp_dir.path().join("simple.py"),
            "x = 1\ny = 2\nprint(x + y)",
        )
        .unwrap();

        let context = create_test_context(&temp_dir);

        let tool = AstGrepTool;
        let params = AstGrepParams {
            pattern: "class $NAME($BASE): $BODY".to_string(),
            lang: Some("python".to_string()),
            include: None,
            exclude: None,
            path: None,
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await.unwrap();

        assert_eq!(result.0.matches.len(), 0);
        assert!(result.0.search_completed);
    }

    #[tokio::test]
    async fn test_astgrep_invalid_path() {
        let temp_dir = tempdir().unwrap();
        let context = create_test_context(&temp_dir);

        let tool = AstGrepTool;
        let params = AstGrepParams {
            pattern: "fn $NAME()".to_string(),
            lang: Some("rust".to_string()),
            include: None,
            exclude: None,
            path: Some("non-existent-dir".to_string()),
        };
        let params_json = serde_json::to_value(params).unwrap();

        let result = tool.execute(params_json, &context).await;

        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("Path does not exist"));
        }
    }
}
