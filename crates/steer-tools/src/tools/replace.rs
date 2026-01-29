use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::{ToolExecutionError, WorkspaceOpError};
use crate::result::ReplaceResult;

pub const REPLACE_TOOL_NAME: &str = "write_file";

pub struct ReplaceToolSpec;

impl ToolSpec for ReplaceToolSpec {
    type Params = ReplaceParams;
    type Result = ReplaceResult;
    type Error = ReplaceError;

    const NAME: &'static str = REPLACE_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Replace File";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::Replace(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum ReplaceError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReplaceParams {
    /// The absolute path to the file to write (must be absolute, not relative)
    pub file_path: String,
    /// The content to write to the file
    pub content: String,
}
