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

pub fn register_builtin_tools(registry: &mut super::ToolRegistry) {
    registry.register_builtin(GrepTool);
    registry.register_builtin(GlobTool);
    registry.register_builtin(LsTool);
    registry.register_builtin(ViewTool);
    registry.register_builtin(BashTool);
    registry.register_builtin(EditTool);
    registry.register_builtin(MultiEditTool);
    registry.register_builtin(ReplaceTool);
    registry.register_builtin(AstGrepTool);
    registry.register_builtin(TodoReadTool);
    registry.register_builtin(TodoWriteTool);
    registry.register_builtin(DispatchAgentTool);
    registry.register_builtin(FetchTool);
}

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
    // This mutates only the session todo list and is intentionally auto-approved.
    steer_tools::tools::TODO_WRITE_TOOL_NAME,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_builtin_tools_registers_all_expected_names() {
        let mut registry = crate::tools::ToolRegistry::new();
        register_builtin_tools(&mut registry);

        let mut names = registry.builtin_tool_names();
        names.sort_unstable();

        let mut expected = vec![
            steer_tools::tools::AST_GREP_TOOL_NAME,
            steer_tools::tools::BASH_TOOL_NAME,
            steer_tools::tools::DISPATCH_AGENT_TOOL_NAME,
            steer_tools::tools::EDIT_TOOL_NAME,
            steer_tools::tools::FETCH_TOOL_NAME,
            steer_tools::tools::GLOB_TOOL_NAME,
            steer_tools::tools::GREP_TOOL_NAME,
            steer_tools::tools::LS_TOOL_NAME,
            steer_tools::tools::MULTI_EDIT_TOOL_NAME,
            steer_tools::tools::REPLACE_TOOL_NAME,
            steer_tools::tools::TODO_READ_TOOL_NAME,
            steer_tools::tools::TODO_WRITE_TOOL_NAME,
            steer_tools::tools::VIEW_TOOL_NAME,
        ];
        expected.sort_unstable();

        assert_eq!(names, expected);
    }
}
