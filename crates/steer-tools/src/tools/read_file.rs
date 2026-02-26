use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::{ToolExecutionError, WorkspaceOpError};
use crate::result::FileContentResult;

pub const READ_FILE_TOOL_NAME: &str = "read_file";

pub struct ReadFileToolSpec;

impl ToolSpec for ReadFileToolSpec {
    type Params = ReadFileParams;
    type Result = FileContentResult;
    type Error = ReadFileError;

    const NAME: &'static str = READ_FILE_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Read File";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::ReadFile(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum ReadFileError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileParams {
    /// The absolute path to the file to read
    pub file_path: String,
    /// The line number to start reading from (1-indexed)
    pub offset: Option<u64>,
    /// The maximum number of lines to read
    pub limit: Option<u64>,
    /// Return raw file bytes rendered as text without numbering/trimming/truncation
    pub raw: Option<bool>,
}
