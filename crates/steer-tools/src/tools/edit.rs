use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::{ToolExecutionError, WorkspaceOpError};
use crate::result::{EditResult, MultiEditResult};
pub use steer_workspace::EditMatchPreview;
use steer_workspace::error::non_unique_match_preview_suffix;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EditMatchMode {
    ExactlyOne,
    First,
    All,
    Nth,
}

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
        "invalid match selection for edit #{edit_index} in file {file_path}: {message}"
    )]
    InvalidMatchSelection {
        file_path: String,
        edit_index: usize,
        message: String,
    },

    #[error(
        "found {occurrences} matches for edit #{edit_index} in file {file_path}; old_string must match exactly once{preview_suffix}",
        preview_suffix = non_unique_match_preview_suffix(match_previews, *omitted_matches)
    )]
    NonUniqueMatch {
        file_path: String,
        edit_index: usize,
        occurrences: usize,
        #[serde(default)]
        match_previews: Vec<EditMatchPreview>,
        #[serde(default)]
        omitted_matches: usize,
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
    /// The exact string to find and replace. Must be non-empty and match according to `match_mode`.
    pub old_string: String,
    /// The string to replace `old_string` with.
    pub new_string: String,
    /// Optional match mode for this edit. Defaults to `exactly_one` when omitted.
    pub match_mode: Option<EditMatchMode>,
    /// Optional 1-based match index used when `match_mode` is `nth`.
    pub match_index: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EditParams {
    /// The absolute path to the file to edit
    pub file_path: String,
    /// The exact string to find and replace. Must be non-empty.
    pub old_string: String,
    /// The string to replace `old_string` with.
    pub new_string: String,
    /// Optional match mode for this edit. Defaults to `exactly_one` when omitted.
    pub match_mode: Option<EditMatchMode>,
    /// Optional 1-based match index used when `match_mode` is `nth`.
    pub match_index: Option<u64>,
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
