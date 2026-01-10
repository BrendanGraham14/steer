pub mod astgrep;
pub mod bash;
pub mod dispatch_agent;
pub mod edit;
pub mod fetch;
pub mod glob;
pub mod grep;
pub mod ls;
pub mod replace;
pub mod todo;
pub mod view;

pub use astgrep::AstGrepTool;
pub use bash::BashTool;
pub use dispatch_agent::DispatchAgentTool;
pub use edit::{EditTool, MultiEditTool};
pub use fetch::FetchTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use replace::ReplaceTool;
pub use todo::{TodoReadTool, TodoWriteTool};
pub use view::ViewTool;

pub(crate) fn workspace_op_error(
    err: steer_workspace::WorkspaceError,
) -> steer_tools::error::WorkspaceOpError {
    use steer_tools::error::WorkspaceOpError;

    match err {
        steer_workspace::WorkspaceError::Io(message) => WorkspaceOpError::Io { message },
        steer_workspace::WorkspaceError::NotSupported(message) => {
            WorkspaceOpError::NotSupported { message }
        }
        other => WorkspaceOpError::Other {
            message: other.to_string(),
        },
    }
}

pub(crate) fn workspace_manager_op_error(
    err: steer_workspace::WorkspaceManagerError,
) -> steer_tools::error::WorkspaceOpError {
    use steer_tools::error::WorkspaceOpError;

    match err {
        steer_workspace::WorkspaceManagerError::NotFound(_) => WorkspaceOpError::NotFound,
        steer_workspace::WorkspaceManagerError::NotSupported(message) => {
            WorkspaceOpError::NotSupported { message }
        }
        steer_workspace::WorkspaceManagerError::Io(message) => WorkspaceOpError::Io { message },
        other => WorkspaceOpError::Other {
            message: other.to_string(),
        },
    }
}

pub const READ_ONLY_TOOL_NAMES: &[&str] = &[
    steer_tools::tools::GREP_TOOL_NAME,
    steer_tools::tools::AST_GREP_TOOL_NAME,
    steer_tools::tools::GLOB_TOOL_NAME,
    steer_tools::tools::LS_TOOL_NAME,
    steer_tools::tools::VIEW_TOOL_NAME,
    steer_tools::tools::TODO_READ_TOOL_NAME,
    steer_tools::tools::TODO_WRITE_TOOL_NAME,
];
