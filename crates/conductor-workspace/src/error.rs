use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorkspaceError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

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

    #[error("Git error: {0}")]
    Git(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),

    #[error("Tonic transport error: {0}")]
    TonicTransport(#[from] tonic::transport::Error),

    #[error("Tonic status error: {0}")]
    TonicStatus(#[from] tonic::Status),
}

pub type Result<T> = std::result::Result<T, WorkspaceError>;
