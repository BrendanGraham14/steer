use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::tools::{
    AST_GREP_TOOL_NAME, BASH_TOOL_NAME, DISPATCH_AGENT_TOOL_NAME, EDIT_TOOL_NAME, FETCH_TOOL_NAME,
    GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, MULTI_EDIT_TOOL_NAME, REPLACE_TOOL_NAME,
    TODO_READ_TOOL_NAME, TODO_WRITE_TOOL_NAME, VIEW_TOOL_NAME, astgrep::AstGrepError,
    bash::BashError, dispatch_agent::DispatchAgentError, edit::EditError,
    edit::multi_edit::MultiEditError, fetch::FetchError, glob::GlobError, grep::GrepError,
    ls::LsError, replace::ReplaceError, todo::read::TodoReadError, todo::write::TodoWriteError,
    view::ViewError,
};

#[derive(Error, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum ToolError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Invalid parameters for {tool_name}: {message}")]
    InvalidParams { tool_name: String, message: String },

    #[error("{0}")]
    Execution(ToolExecutionError),

    #[error("{0} was cancelled")]
    Cancelled(String),

    #[error("{0} timed out")]
    Timeout(String),

    #[error("{0} requires approval to run")]
    DeniedByUser(String),

    #[error("{0} denied by approval policy")]
    DeniedByPolicy(String),

    #[error("Unexpected error: {0}")]
    InternalError(String),
}

impl ToolError {
    pub fn execution<T: Into<String>, M: Into<String>>(tool_name: T, message: M) -> Self {
        ToolError::Execution(ToolExecutionError::External {
            tool_name: tool_name.into(),
            message: message.into(),
        })
    }

    pub fn invalid_params<T: Into<String>, M: Into<String>>(tool_name: T, message: M) -> Self {
        ToolError::InvalidParams {
            tool_name: tool_name.into(),
            message: message.into(),
        }
    }
}

#[derive(Error, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "tool", content = "error", rename_all = "snake_case")]
pub enum ToolExecutionError {
    #[error("{0}")]
    AstGrep(AstGrepError),
    #[error("{0}")]
    Bash(BashError),
    #[error("{0}")]
    Edit(EditError),
    #[error("{0}")]
    MultiEdit(MultiEditError),
    #[error("{0}")]
    Fetch(FetchError),
    #[error("{0}")]
    Glob(GlobError),
    #[error("{0}")]
    Grep(GrepError),
    #[error("{0}")]
    Ls(LsError),
    #[error("{0}")]
    Replace(ReplaceError),
    #[error("{0}")]
    TodoRead(TodoReadError),
    #[error("{0}")]
    TodoWrite(TodoWriteError),
    #[error("{0}")]
    View(ViewError),
    #[error("{0}")]
    DispatchAgent(DispatchAgentError),

    #[error("{tool_name} failed: {message}")]
    External { tool_name: String, message: String },
}

impl ToolExecutionError {
    pub fn tool_name(&self) -> &str {
        match self {
            ToolExecutionError::AstGrep(_) => AST_GREP_TOOL_NAME,
            ToolExecutionError::Bash(_) => BASH_TOOL_NAME,
            ToolExecutionError::Edit(_) => EDIT_TOOL_NAME,
            ToolExecutionError::MultiEdit(_) => MULTI_EDIT_TOOL_NAME,
            ToolExecutionError::Fetch(_) => FETCH_TOOL_NAME,
            ToolExecutionError::Glob(_) => GLOB_TOOL_NAME,
            ToolExecutionError::Grep(_) => GREP_TOOL_NAME,
            ToolExecutionError::Ls(_) => LS_TOOL_NAME,
            ToolExecutionError::Replace(_) => REPLACE_TOOL_NAME,
            ToolExecutionError::TodoRead(_) => TODO_READ_TOOL_NAME,
            ToolExecutionError::TodoWrite(_) => TODO_WRITE_TOOL_NAME,
            ToolExecutionError::View(_) => VIEW_TOOL_NAME,
            ToolExecutionError::DispatchAgent(_) => DISPATCH_AGENT_TOOL_NAME,
            ToolExecutionError::External { tool_name, .. } => tool_name.as_str(),
        }
    }
}

#[derive(Error, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum WorkspaceOpError {
    #[error("path is outside workspace")]
    InvalidPath,

    #[error("file not found")]
    NotFound,

    #[error("permission denied")]
    PermissionDenied,

    #[error("operation not supported: {message}")]
    NotSupported { message: String },

    #[error("io error: {message}")]
    Io { message: String },

    #[error("{message}")]
    Other { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::Value;

    fn assert_workspace_error_roundtrip<T>(error: T)
    where
        T: Serialize + DeserializeOwned + std::fmt::Debug,
    {
        let serialized = serde_json::to_string(&error).expect("serialize error");
        let value: Value = serde_json::from_str(&serialized).expect("deserialize json");
        let obj = match &value {
            Value::Object(map) => map,
            other => panic!("expected object, got {other:?}"),
        };
        assert_eq!(obj.len(), 2);
        assert!(obj.contains_key("code"));
        assert!(obj.contains_key("details"));
        assert_eq!(
            value.get("code"),
            Some(&Value::String("workspace".to_string()))
        );
        let details = value.get("details").expect("workspace details missing");
        assert_eq!(
            details.get("code"),
            Some(&Value::String("not_found".to_string()))
        );
        let _: T = serde_json::from_str(&serialized).expect("roundtrip error");
    }

    #[test]
    fn workspace_error_wrappers_roundtrip() {
        let workspace_error = WorkspaceOpError::NotFound;
        assert_workspace_error_roundtrip(AstGrepError::Workspace(workspace_error.clone()));
        assert_workspace_error_roundtrip(EditError::Workspace(workspace_error.clone()));
        assert_workspace_error_roundtrip(MultiEditError::Workspace(workspace_error.clone()));
        assert_workspace_error_roundtrip(GlobError::Workspace(workspace_error.clone()));
        assert_workspace_error_roundtrip(GrepError::Workspace(workspace_error.clone()));
        assert_workspace_error_roundtrip(LsError::Workspace(workspace_error.clone()));
        assert_workspace_error_roundtrip(ReplaceError::Workspace(workspace_error.clone()));
        assert_workspace_error_roundtrip(ViewError::Workspace(workspace_error.clone()));
        assert_workspace_error_roundtrip(DispatchAgentError::Workspace(workspace_error));
    }
}
