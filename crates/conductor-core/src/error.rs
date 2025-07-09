use thiserror::Error;

use crate::{
    api::ApiError,
    app::AgentExecutorError,
    auth::AuthError,
    session::{manager::SessionManagerError, store::SessionStoreError},
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    AgentExecutor(#[from] AgentExecutorError),
    #[error(transparent)]
    SessionManager(#[from] SessionManagerError),
    #[error(transparent)]
    SessionStore(#[from] SessionStoreError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Tool(#[from] conductor_tools::ToolError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    Configuration(String),
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Cancelled")]
    Cancelled,
    #[error("Ignore error: {0}")]
    Ignore(#[from] ignore::Error),
    #[error("gRPC transport error: {0}")]
    Transport(String),
    #[error("gRPC status error: {0}")]
    Status(String),
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("Workspace not supported: {0}")]
    NotSupported(String),
    #[error("Failed to collect environment: {0}")]
    EnvironmentCollection(String),
    #[error("Tool execution failed: {0}")]
    ToolExecution(String),
    #[error("Invalid workspace path: {0}")]
    InvalidPath(String),
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error("Remote workspace error: {0}")]
    Remote(String),
}
