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

    #[error("I/O error during tool execution for {tool_name}: {source}")]
    Io {
        tool_name: String,
        #[source]
        source: anyhow::Error, // Can wrap std::io::Error or others via anyhow
    },
    // Add other specific error types as needed
}

// Helper to convert anyhow::Error to ToolError::Execution
impl ToolError {
    pub fn execution<S: Into<String>>(tool_name: S, err: anyhow::Error) -> Self {
        ToolError::Execution {
            tool_name: tool_name.into(),
            message: err.to_string(),
        }
    }
    pub fn io<S: Into<String>>(tool_name: S, err: anyhow::Error) -> Self {
        ToolError::Io {
            tool_name: tool_name.into(),
            source: err,
        }
    }
}
