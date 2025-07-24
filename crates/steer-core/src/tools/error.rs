use crate::tools::mcp::McpError;
use serde::{Deserialize, Serialize};
use steer_tools::ToolError as ToolExecutionError;
use steer_workspace::WorkspaceError;
use thiserror::Error;

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum ToolError {
    #[error(transparent)]
    Execution(#[from] ToolExecutionError),

    #[error(transparent)]
    Mcp(#[from] McpError),

    #[error("Invalid pattern: {0}")]
    Regex(String),

    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
}

pub type Result<T> = std::result::Result<T, ToolError>;
