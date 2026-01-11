use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::{ToolExecutionError, WorkspaceOpError};
use crate::result::GlobResult;

pub const GLOB_TOOL_NAME: &str = "glob";

pub struct GlobToolSpec;

impl ToolSpec for GlobToolSpec {
    type Params = GlobParams;
    type Result = GlobResult;
    type Error = GlobError;

    const NAME: &'static str = GLOB_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Glob";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::Glob(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum GlobError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GlobParams {
    /// The glob pattern to match files against
    pub pattern: String,
    /// Optional directory to search in. Defaults to the current working directory.
    pub path: Option<String>,
}
