use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::WorkspaceOpError;
use crate::result::GrepResult;

pub const GREP_TOOL_NAME: &str = "grep";

pub struct GrepToolSpec;

impl ToolSpec for GrepToolSpec {
    type Params = GrepParams;
    type Result = GrepResult;
    type Error = GrepError;

    const NAME: &'static str = GREP_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Grep";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum GrepError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GrepParams {
    /// The search pattern (regex or literal string). If invalid regex, searches for literal text
    pub pattern: String,
    /// Optional glob pattern to filter files by name (e.g., "*.rs", "*.{ts,tsx}")
    pub include: Option<String>,
    /// Optional directory to search in (defaults to current working directory)
    pub path: Option<String>,
}
