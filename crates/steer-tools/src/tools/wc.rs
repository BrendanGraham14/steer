use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::{ToolExecutionError, WorkspaceOpError};
use crate::result::WcResult;

pub const WC_TOOL_NAME: &str = "wc";

pub struct WcToolSpec;

impl ToolSpec for WcToolSpec {
    type Params = WcParams;
    type Result = WcResult;
    type Error = WcError;

    const NAME: &'static str = WC_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Word Count";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::Wc(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum WcError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WcParams {
    /// Path to the file to measure (absolute path, or relative to the workspace root)
    pub file_path: String,
}
