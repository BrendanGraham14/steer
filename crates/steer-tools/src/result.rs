use crate::{
    error::ToolError,
    tools::todo::{TodoItem, TodoWriteFileOperation},
};
use serde::{Deserialize, Serialize};

pub use steer_workspace::result::{
    EditResult, FileContentResult, FileEntry, FileListResult, GlobResult, SearchMatch, SearchResult,
};

/// Core enum for all tool results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolResult {
    // One variant per built-in tool
    Search(SearchResult),     // grep / astgrep
    FileList(FileListResult), // ls / glob
    FileContent(FileContentResult),
    Edit(EditResult),
    Bash(BashResult),
    Glob(GlobResult),
    TodoRead(TodoListResult),
    TodoWrite(TodoWriteResult),
    Fetch(FetchResult),
    Agent(AgentResult),

    // Unknown or remote (MCP) tool payload
    External(ExternalResult),

    // Failure (any tool)
    Error(ToolError),
}

/// Result for the fetch tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    pub url: String,
    pub content: String,
}

/// Workspace revision metadata for dispatched agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentWorkspaceRevision {
    pub vcs_kind: String,
    pub revision_id: String,
    pub summary: String,
    #[serde(default)]
    pub change_id: Option<String>,
}

/// Workspace context for dispatched agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentWorkspaceInfo {
    pub workspace_id: Option<String>,
    pub revision: Option<AgentWorkspaceRevision>,
}

/// Result for the dispatch_agent tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub content: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub workspace: Option<AgentWorkspaceInfo>,
}

/// Result for external/MCP tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalResult {
    pub tool_name: String, // name reported by the MCP server
    pub payload: String,   // raw, opaque blob (usually JSON or text)
}

/// Result for bash command execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub command: String,
}

/// Result for todo operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoListResult {
    pub todos: Vec<TodoItem>,
}
/// Result for todo write operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoWriteResult {
    pub todos: Vec<TodoItem>,
    pub operation: TodoWriteFileOperation,
}

// Newtype wrappers to avoid conflicting From impls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiEditResult(pub EditResult);
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceResult(pub EditResult);
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstGrepResult(pub SearchResult);
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepResult(pub SearchResult);

// Trait for typed tool outputs
pub trait ToolOutput: Serialize + Send + Sync + 'static {}

// Implement ToolOutput for all result types
impl ToolOutput for SearchResult {}
impl ToolOutput for GrepResult {}
impl ToolOutput for FileListResult {}
impl ToolOutput for FileContentResult {}
impl ToolOutput for EditResult {}
impl ToolOutput for BashResult {}
impl ToolOutput for GlobResult {}
impl ToolOutput for TodoListResult {}
impl ToolOutput for TodoWriteResult {}
impl ToolOutput for MultiEditResult {}
impl ToolOutput for ReplaceResult {}
impl ToolOutput for AstGrepResult {}
impl ToolOutput for ExternalResult {}
impl ToolOutput for FetchResult {}
impl ToolOutput for AgentResult {}
impl ToolOutput for ToolResult {}

// Manual From implementations to support StaticTool::Output conversions
impl From<SearchResult> for ToolResult {
    fn from(r: SearchResult) -> Self {
        Self::Search(r)
    }
}

impl From<GrepResult> for ToolResult {
    fn from(r: GrepResult) -> Self {
        Self::Search(r.0)
    }
}

impl From<AstGrepResult> for ToolResult {
    fn from(r: AstGrepResult) -> Self {
        Self::Search(r.0)
    }
}

impl From<FileListResult> for ToolResult {
    fn from(r: FileListResult) -> Self {
        Self::FileList(r)
    }
}

impl From<FileContentResult> for ToolResult {
    fn from(r: FileContentResult) -> Self {
        Self::FileContent(r)
    }
}

impl From<EditResult> for ToolResult {
    fn from(r: EditResult) -> Self {
        Self::Edit(r)
    }
}

impl From<MultiEditResult> for ToolResult {
    fn from(r: MultiEditResult) -> Self {
        Self::Edit(r.0)
    }
}

impl From<ReplaceResult> for ToolResult {
    fn from(r: ReplaceResult) -> Self {
        Self::Edit(r.0)
    }
}

impl From<BashResult> for ToolResult {
    fn from(r: BashResult) -> Self {
        Self::Bash(r)
    }
}

impl From<GlobResult> for ToolResult {
    fn from(r: GlobResult) -> Self {
        Self::Glob(r)
    }
}

impl From<TodoListResult> for ToolResult {
    fn from(r: TodoListResult) -> Self {
        Self::TodoRead(r)
    }
}

impl From<TodoWriteResult> for ToolResult {
    fn from(r: TodoWriteResult) -> Self {
        Self::TodoWrite(r)
    }
}

impl From<FetchResult> for ToolResult {
    fn from(r: FetchResult) -> Self {
        Self::Fetch(r)
    }
}

impl From<AgentResult> for ToolResult {
    fn from(r: AgentResult) -> Self {
        Self::Agent(r)
    }
}

impl From<ExternalResult> for ToolResult {
    fn from(r: ExternalResult) -> Self {
        Self::External(r)
    }
}

impl From<ToolError> for ToolResult {
    fn from(e: ToolError) -> Self {
        Self::Error(e)
    }
}

impl ToolResult {
    /// Format the result for LLM consumption
    pub fn llm_format(&self) -> String {
        match self {
            ToolResult::Search(r) => {
                if r.matches.is_empty() {
                    "No matches found.".to_string()
                } else {
                    let mut output = Vec::new();
                    let mut current_file = "";

                    for match_item in &r.matches {
                        if match_item.file_path != current_file {
                            if !output.is_empty() {
                                output.push(String::new());
                            }
                            current_file = &match_item.file_path;
                        }
                        output.push(format!(
                            "{}:{}: {}",
                            match_item.file_path, match_item.line_number, match_item.line_content
                        ));
                    }

                    output.join("\n")
                }
            }
            ToolResult::FileList(r) => {
                if r.entries.is_empty() {
                    format!("No entries found in {}", r.base_path)
                } else {
                    let mut lines = Vec::new();
                    for entry in &r.entries {
                        let type_indicator = if entry.is_directory { "/" } else { "" };
                        let size_str = entry.size.map(|s| format!(" ({s})")).unwrap_or_default();
                        lines.push(format!("{}{}{}", entry.path, type_indicator, size_str));
                    }
                    lines.join("\n")
                }
            }
            ToolResult::FileContent(r) => r.content.clone(),
            ToolResult::Edit(r) => {
                if r.file_created {
                    format!("Successfully created {}", r.file_path)
                } else {
                    format!(
                        "Successfully edited {}: {} change(s) made",
                        r.file_path, r.changes_made
                    )
                }
            }
            ToolResult::Bash(r) => {
                // Helper to truncate long outputs
                fn truncate_output(s: &str, max_chars: usize, max_lines: usize) -> String {
                    let lines: Vec<&str> = s.lines().collect();
                    let char_count = s.len();

                    // Check both line and character limits
                    if lines.len() > max_lines || char_count > max_chars {
                        // Take first and last portions of output
                        let head_lines = max_lines / 2;
                        let tail_lines = max_lines - head_lines;

                        let mut result = String::new();

                        // Add head lines
                        for line in lines.iter().take(head_lines) {
                            result.push_str(line);
                            result.push('\n');
                        }

                        // Add truncation marker
                        let omitted_lines = lines.len().saturating_sub(max_lines);
                        result.push_str(&format!(
                            "\n[... {omitted_lines} lines omitted ({char_count} total chars) ...]\n\n"
                        ));

                        // Add tail lines
                        if tail_lines > 0 && lines.len() > head_lines {
                            for line in lines.iter().skip(lines.len().saturating_sub(tail_lines)) {
                                result.push_str(line);
                                result.push('\n');
                            }
                        }

                        result
                    } else {
                        s.to_string()
                    }
                }

                const MAX_STDOUT_CHARS: usize = 128 * 1024; // 128KB
                const MAX_STDOUT_LINES: usize = 2000;
                const MAX_STDERR_CHARS: usize = 64 * 1024; // 64KB  
                const MAX_STDERR_LINES: usize = 500;

                let stdout_truncated =
                    truncate_output(&r.stdout, MAX_STDOUT_CHARS, MAX_STDOUT_LINES);
                let stderr_truncated =
                    truncate_output(&r.stderr, MAX_STDERR_CHARS, MAX_STDERR_LINES);

                let mut output = stdout_truncated;

                if r.exit_code != 0 {
                    if !output.is_empty() && !output.ends_with('\n') {
                        output.push('\n');
                    }
                    output.push_str(&format!("Exit code: {}", r.exit_code));

                    if !stderr_truncated.is_empty() {
                        output.push_str(&format!("\nError output:\n{stderr_truncated}"));
                    }
                } else if !stderr_truncated.is_empty() {
                    if !output.is_empty() && !output.ends_with('\n') {
                        output.push('\n');
                    }
                    output.push_str(&format!("Error output:\n{stderr_truncated}"));
                }

                output
            }
            ToolResult::Glob(r) => {
                if r.matches.is_empty() {
                    format!("No files matching pattern: {}", r.pattern)
                } else {
                    r.matches.join("\n")
                }
            }
            ToolResult::TodoRead(r) => {
                if r.todos.is_empty() {
                    "No todos found.".to_string()
                } else {
                    format!(
                        "Remember to continue to update and read from the todo list as you make progress. Here is the current list:\n{}",
                        serde_json::to_string_pretty(&r.todos)
                            .unwrap_or_else(|_| "Failed to format todos".to_string())
                    )
                }
            }
            ToolResult::TodoWrite(r) => {
                format!(
                    "Todos have been {:?} successfully. Ensure that you continue to read and update the todo list as you work on tasks.\n{}",
                    r.operation,
                    serde_json::to_string_pretty(&r.todos)
                        .unwrap_or_else(|_| "Failed to format todos".to_string())
                )
            }
            ToolResult::Fetch(r) => {
                format!("Fetched content from {}:\n{}", r.url, r.content)
            }
            ToolResult::Agent(r) => r.session_id.as_ref().map_or_else(
                || r.content.clone(),
                |session_id| format!("{}\n\nsession_id: {}", r.content, session_id),
            ),
            ToolResult::External(r) => r.payload.clone(),
            ToolResult::Error(e) => format!("Error: {e}"),
        }
    }

    /// Get the variant name as a string for metadata
    pub fn variant_name(&self) -> &'static str {
        match self {
            ToolResult::Search(_) => "Search",
            ToolResult::FileList(_) => "FileList",
            ToolResult::FileContent(_) => "FileContent",
            ToolResult::Edit(_) => "Edit",
            ToolResult::Bash(_) => "Bash",
            ToolResult::Glob(_) => "Glob",
            ToolResult::TodoRead(_) => "TodoRead",
            ToolResult::TodoWrite(_) => "TodoWrite",
            ToolResult::Fetch(_) => "Fetch",
            ToolResult::Agent(_) => "Agent",
            ToolResult::External(_) => "External",
            ToolResult::Error(_) => "Error",
        }
    }
}
