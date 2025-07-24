use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceError {
    #[error("I/O error: {0}")]
    Io(String),

    #[error("Tool execution failed: {0}")]
    ToolExecution(String),

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Status error: {0}")]
    Status(String),

    #[error("Not supported: {0}")]
    NotSupported(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    #[error("Remote workspace error: {0}")]
    Remote(String),
}

pub type Result<T> = std::result::Result<T, WorkspaceError>;

impl From<tonic::transport::Error> for WorkspaceError {
    fn from(err: tonic::transport::Error) -> Self {
        WorkspaceError::Transport(err.to_string())
    }
}

impl From<tonic::Status> for WorkspaceError {
    fn from(err: tonic::Status) -> Self {
        WorkspaceError::Status(err.to_string())
    }
}

impl From<std::io::Error> for WorkspaceError {
    fn from(err: std::io::Error) -> Self {
        WorkspaceError::Io(err.to_string())
    }
}
