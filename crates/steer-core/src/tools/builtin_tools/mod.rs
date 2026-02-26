pub mod astgrep;
pub mod bash;
pub mod dispatch_agent;
pub mod edit;
pub mod fetch;
pub mod glob;
pub mod grep;
pub mod ls;
pub mod read_file;
pub mod replace;
pub mod todo;

pub use astgrep::AstGrepTool;
pub use bash::BashTool;
pub use dispatch_agent::DispatchAgentTool;
pub use edit::{EditTool, MultiEditTool};
pub use fetch::FetchTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use read_file::ReadFileTool;
pub use replace::ReplaceTool;
pub use todo::{TodoReadTool, TodoWriteTool};

use crate::session::state::ToolVisibility;

#[cfg(test)]
pub(crate) const ALL_BUILTIN_TOOL_NAMES: &[&str] = &[
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
    steer_tools::tools::READ_FILE_TOOL_NAME,
];

pub(crate) fn register_builtin_tools(registry: &mut super::ToolRegistry) {
    register_builtin_tools_for_visibility(registry, &ToolVisibility::All);
}

pub(crate) fn register_builtin_tools_for_visibility(
    registry: &mut super::ToolRegistry,
    visibility: &ToolVisibility,
) {
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::GREP_TOOL_NAME,
        |registry| {
            registry.register_builtin(GrepTool);
        },
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::GLOB_TOOL_NAME,
        |registry| {
            registry.register_builtin(GlobTool);
        },
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::LS_TOOL_NAME,
        |registry| {
            registry.register_builtin(LsTool);
        },
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::READ_FILE_TOOL_NAME,
        |registry| {
            registry.register_builtin(ReadFileTool);
        },
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::BASH_TOOL_NAME,
        |registry| {
            registry.register_builtin(BashTool);
        },
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::EDIT_TOOL_NAME,
        |registry| {
            registry.register_builtin(EditTool);
        },
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::MULTI_EDIT_TOOL_NAME,
        |registry| registry.register_builtin(MultiEditTool),
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::REPLACE_TOOL_NAME,
        |registry| registry.register_builtin(ReplaceTool),
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::AST_GREP_TOOL_NAME,
        |registry| registry.register_builtin(AstGrepTool),
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::TODO_READ_TOOL_NAME,
        |registry| registry.register_builtin(TodoReadTool),
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::TODO_WRITE_TOOL_NAME,
        |registry| registry.register_builtin(TodoWriteTool),
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::DISPATCH_AGENT_TOOL_NAME,
        |registry| registry.register_builtin(DispatchAgentTool),
    );
    register_if_visible(
        registry,
        visibility,
        steer_tools::tools::FETCH_TOOL_NAME,
        |registry| {
            registry.register_builtin(FetchTool);
        },
    );
}

fn register_if_visible(
    registry: &mut super::ToolRegistry,
    visibility: &ToolVisibility,
    tool_name: &str,
    register: impl FnOnce(&mut super::ToolRegistry),
) {
    if builtin_tool_is_visible(tool_name, visibility) {
        register(registry);
    }
}

fn builtin_tool_is_visible(tool_name: &str, visibility: &ToolVisibility) -> bool {
    match visibility {
        ToolVisibility::All => true,
        ToolVisibility::ReadOnly => READ_ONLY_TOOL_NAMES.contains(&tool_name),
        ToolVisibility::Whitelist(allowed) => allowed.contains(tool_name),
        ToolVisibility::Blacklist(blocked) => !blocked.contains(tool_name),
    }
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
    steer_tools::tools::READ_FILE_TOOL_NAME,
    steer_tools::tools::TODO_READ_TOOL_NAME,
    // This mutates only the session todo list and is intentionally auto-approved.
    steer_tools::tools::TODO_WRITE_TOOL_NAME,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn register_builtin_tools_registers_all_expected_names() {
        let mut registry = crate::tools::ToolRegistry::new();
        register_builtin_tools(&mut registry);

        let mut names = registry.builtin_tool_names();
        names.sort_unstable();

        let mut expected = ALL_BUILTIN_TOOL_NAMES.to_vec();
        expected.sort_unstable();

        assert_eq!(names, expected);
    }

    #[test]
    fn register_builtin_tools_for_visibility_honors_whitelist() {
        let mut registry = crate::tools::ToolRegistry::new();
        let visibility = ToolVisibility::Whitelist(HashSet::from([
            steer_tools::tools::READ_FILE_TOOL_NAME.to_string(),
            steer_tools::tools::TODO_READ_TOOL_NAME.to_string(),
        ]));

        register_builtin_tools_for_visibility(&mut registry, &visibility);

        let mut names = registry.builtin_tool_names();
        names.sort_unstable();

        let mut expected = vec![
            steer_tools::tools::TODO_READ_TOOL_NAME,
            steer_tools::tools::READ_FILE_TOOL_NAME,
        ];
        expected.sort_unstable();

        assert_eq!(names, expected);
    }
}
