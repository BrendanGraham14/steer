use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::{ToolExecutionError, WorkspaceOpError};
use crate::result::{EditResult, MultiEditResult};

pub const EDIT_TOOL_NAME: &str = "edit_file";

pub struct EditToolSpec;

impl ToolSpec for EditToolSpec {
    type Params = EditParams;
    type Result = EditResult;
    type Error = EditError;

    const NAME: &'static str = EDIT_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Edit File";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::Edit(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum EditFailure {
    #[error("file not found: {file_path}")]
    FileNotFound { file_path: String },

    #[error(
        "edit #{edit_index} has an empty old_string; use write_file to create or overwrite files"
    )]
    EmptyOldString { edit_index: usize },

    #[error("string not found for edit #{edit_index} in file {file_path}")]
    StringNotFound {
        file_path: String,
        edit_index: usize,
    },

    #[error(
        "found {occurrences} matches for edit #{edit_index} in file {file_path}; old_string must match exactly once"
    )]
    NonUniqueMatch {
        file_path: String,
        edit_index: usize,
        occurrences: usize,
    },
}

#[derive(Deserialize, Serialize, Debug, JsonSchema, Clone, Error)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum EditError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),

    #[error("{0}")]
    EditFailure(EditFailure),
}

#[derive(Deserialize, Serialize, Debug, JsonSchema, Clone)]
pub struct SingleEditOperation {
    /// The exact string to find and replace. Must be non-empty and match exactly one location.
    pub old_string: String,
    /// The string to replace `old_string` with.
    pub new_string: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EditParams {
    /// The absolute path to the file to edit
    pub file_path: String,
    /// The exact string to find and replace. Must be non-empty.
    pub old_string: String,
    /// The string to replace `old_string` with.
    pub new_string: String,
}

pub mod multi_edit {
    use super::{
        Deserialize, EditFailure, Error, JsonSchema, MultiEditResult, Serialize,
        SingleEditOperation, ToolExecutionError, ToolSpec, WorkspaceOpError,
    };

    pub const MULTI_EDIT_TOOL_NAME: &str = "multi_edit";

    pub struct MultiEditToolSpec;

    impl ToolSpec for MultiEditToolSpec {
        type Params = MultiEditParams;
        type Result = MultiEditResult;
        type Error = MultiEditError;

        const NAME: &'static str = MULTI_EDIT_TOOL_NAME;
        const DISPLAY_NAME: &'static str = "Multi Edit";

        fn execution_error(error: Self::Error) -> ToolExecutionError {
            ToolExecutionError::MultiEdit(error)
        }
    }

    #[derive(Deserialize, Serialize, Debug, JsonSchema, Clone, Error)]
    #[serde(tag = "code", content = "details", rename_all = "snake_case")]
    pub enum MultiEditError {
        #[error("{0}")]
        Workspace(WorkspaceOpError),

        #[error("{0}")]
        EditFailure(EditFailure),
    }

    #[derive(Deserialize, Serialize, Debug, JsonSchema)]
    pub struct MultiEditParams {
        /// The absolute path to the file to edit.
        pub file_path: String,
        /// A list of edit operations to apply sequentially.
        pub edits: Vec<SingleEditOperation>,
    }
}
