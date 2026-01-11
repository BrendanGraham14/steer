use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::WorkspaceOpError;
use crate::result::FileListResult;

pub const LS_TOOL_NAME: &str = "ls";

pub struct LsToolSpec;

impl ToolSpec for LsToolSpec {
    type Params = LsParams;
    type Result = FileListResult;
    type Error = LsError;

    const NAME: &'static str = LS_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "List Files";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum LsError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LsParams {
    /// The absolute path to the directory to list (must be absolute, not relative)
    pub path: String,
    /// Optional list of glob patterns to ignore
    pub ignore: Option<Vec<String>>,
}
