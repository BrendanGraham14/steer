use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::WorkspaceOpError;

pub const LS_TOOL_NAME: &str = "ls";

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
