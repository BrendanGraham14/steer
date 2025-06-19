use crate::Tool;
use crate::tools::{
    BashTool, EditTool, GlobTool, GrepTool, LsTool, MultiEditTool, ReplaceTool, TodoReadTool,
    TodoWriteTool, ViewTool,
};

/// Tools that operate on the workspace (files, execution, etc.)
///
/// This includes all file manipulation and execution tools that work
/// in the context of a workspace/filesystem.
pub fn workspace_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(BashTool),
        Box::new(GrepTool),
        Box::new(GlobTool),
        Box::new(LsTool),
        Box::new(ViewTool),
        Box::new(EditTool),
        Box::new(MultiEditTool),
        Box::new(ReplaceTool),
        Box::new(TodoReadTool),
        Box::new(TodoWriteTool),
    ]
}

/// Read-only workspace tools
///
/// This includes only tools that read information without modifying files.
/// These are safe to use in restricted environments.
/// Note: TodoReadTool and TodoWriteTool are included as they only modify
/// in-memory session state, not actual files.
pub fn read_only_workspace_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(GrepTool),
        Box::new(GlobTool),
        Box::new(LsTool),
        Box::new(ViewTool),
        Box::new(TodoReadTool),
        Box::new(TodoWriteTool),
    ]
}

/// Server-side tools that don't operate on the workspace
///
/// These tools provide capabilities that are specific to the client/server
/// and don't require access to the workspace filesystem.
/// Note: These are NOT included in the tools crate, but this function
/// documents what would be considered server tools.
pub fn server_tools_note() -> &'static str {
    "Server tools like FetchTool and DispatchAgentTool are defined in the conductor crate"
}
