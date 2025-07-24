use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum McpError {
    #[error("Cannot connect to {server_name}: {message}")]
    ConnectionFailed {
        server_name: String,
        message: String,
    },

    #[error("Failed to list tools: {message}")]
    ListToolsFailed { message: String },

    #[error("Failed to serve MCP over {transport}: {message}")]
    ServeFailed { transport: String, message: String },

    #[error("Timeout listing tools from {server_name}")]
    ListToolsTimeout { server_name: String },
}
