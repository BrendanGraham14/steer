use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum ToolError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Invalid parameters for {0}: {1}")]
    InvalidParams(String, String), // Tool name, error message

    #[error("{tool_name} failed: {message}")]
    Execution { tool_name: String, message: String },

    #[error("{0} was cancelled")]
    Cancelled(String), // Tool name or ID

    #[error("{0} timed out")]
    Timeout(String), // Tool name or ID

    #[error("{0} requires approval to run")]
    DeniedByUser(String), // Tool name

    #[error("Unexpected error: {0}")]
    InternalError(String), // Error message

    #[error("File operation failed in {tool_name}: {message}")]
    Io { tool_name: String, message: String },

    #[error("Failed to process data: {0}")]
    Serialization(String),

    #[error("Network request failed: {0}")]
    Http(String),

    #[error("Invalid pattern: {0}")]
    Regex(String),

    #[error("Cannot connect to {server_name}: {message}")]
    McpConnectionFailed {
        server_name: String,
        message: String,
    },
}

impl ToolError {
    pub fn execution<T: Into<String>, M: Into<String>>(tool_name: T, message: M) -> Self {
        ToolError::Execution {
            tool_name: tool_name.into(),
            message: message.into(),
        }
    }

    pub fn io<T: Into<String>, M: Into<String>>(tool_name: T, message: M) -> Self {
        ToolError::Io {
            tool_name: tool_name.into(),
            message: message.into(),
        }
    }

    pub fn invalid_params<T: Into<String>, M: Into<String>>(tool_name: T, message: M) -> Self {
        ToolError::InvalidParams(tool_name.into(), message.into())
    }

    pub fn mcp_connection_failed<T: Into<String>, M: Into<String>>(
        server_name: T,
        message: M,
    ) -> Self {
        ToolError::McpConnectionFailed {
            server_name: server_name.into(),
            message: message.into(),
        }
    }
}
