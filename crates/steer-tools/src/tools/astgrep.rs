use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::{ToolExecutionError, WorkspaceOpError};
use crate::result::AstGrepResult;

pub const AST_GREP_TOOL_NAME: &str = "astgrep";

pub struct AstGrepToolSpec;

impl ToolSpec for AstGrepToolSpec {
    type Params = AstGrepParams;
    type Result = AstGrepResult;
    type Error = AstGrepError;

    const NAME: &'static str = AST_GREP_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "AST Grep";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::AstGrep(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum AstGrepError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AstGrepParams {
    /// The search pattern (code pattern with $METAVAR placeholders)
    pub pattern: String,
    /// Language (rust, tsx, python, etc.)
    pub lang: Option<String>,
    /// Optional glob pattern to filter files by name (e.g., "*.rs", "*.{ts,tsx}")
    pub include: Option<String>,
    /// Optional glob pattern to exclude files
    pub exclude: Option<String>,
    /// Optional directory to search in (defaults to current working directory)
    pub path: Option<String>,
}
