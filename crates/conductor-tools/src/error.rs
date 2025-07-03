use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum ToolError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Invalid parameters for tool {0}: {1}")]
    InvalidParams(String, String), // Tool name, error message

    #[error("Tool execution failed for {tool_name}: {message}")]
    Execution { tool_name: String, message: String },

    #[error("Tool execution cancelled: {0}")]
    Cancelled(String), // Tool name or ID

    #[error("Tool execution timed out: {0}")]
    Timeout(String), // Tool name or ID

    #[error("Tool execution denied by user: {0}")]
    DeniedByUser(String), // Tool name

    #[error("Internal error during tool execution: {0}")]
    InternalError(String), // Error message

    #[error("I/O error during tool execution for {tool_name}: {message}")]
    Io { tool_name: String, message: String },

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Regex error: {0}")]
    Regex(String),

    #[error("MCP server connection failed for {server_name}: {message}")]
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
