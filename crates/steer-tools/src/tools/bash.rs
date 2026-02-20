use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::ToolExecutionError;
use crate::result::BashResult;

pub const BASH_TOOL_NAME: &str = "bash";

pub struct BashToolSpec;

impl ToolSpec for BashToolSpec {
    type Params = BashParams;
    type Result = BashResult;
    type Error = BashError;

    const NAME: &'static str = BASH_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Bash";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::Bash(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum BashError {
    #[error("command is disallowed: {command}")]
    DisallowedCommand { command: String },

    #[error("io error: {message}")]
    Io { message: String },

    #[error("{message}")]
    Other { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BashParams {
    /// The command to execute
    pub command: String,
    /// Optional timeout in milliseconds (default 180000, max 3600000)
    #[schemars(range(min = 1, max = 3_600_000))]
    pub timeout: Option<u64>,
}
