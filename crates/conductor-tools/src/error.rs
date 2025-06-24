use thiserror::Error;

#[derive(Error, Debug)]
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
    Serialization(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),
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
}
