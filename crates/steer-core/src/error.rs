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
    Tool(#[from] steer_tools::ToolError),
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
    Git(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),
    #[error("Remote workspace error: {0}")]
    Remote(String),
}

impl From<steer_workspace::WorkspaceError> for WorkspaceError {
    fn from(err: steer_workspace::WorkspaceError) -> Self {
        match err {
            steer_workspace::WorkspaceError::NotSupported(msg) => WorkspaceError::NotSupported(msg),
            steer_workspace::WorkspaceError::ToolExecution(msg) => {
                WorkspaceError::ToolExecution(msg)
            }
            steer_workspace::WorkspaceError::Git(e) => WorkspaceError::Git(e),
            steer_workspace::WorkspaceError::Io(e) => {
                WorkspaceError::EnvironmentCollection(e.to_string())
            }
            steer_workspace::WorkspaceError::Transport(msg) => WorkspaceError::Remote(msg),
            steer_workspace::WorkspaceError::Status(msg) => WorkspaceError::Remote(msg),
            steer_workspace::WorkspaceError::InvalidConfiguration(msg) => {
                WorkspaceError::InvalidPath(msg)
            }
            steer_workspace::WorkspaceError::TonicTransport(e) => {
                WorkspaceError::Remote(e.to_string())
            }
            steer_workspace::WorkspaceError::TonicStatus(e) => {
                WorkspaceError::Remote(e.to_string())
            }
        }
    }
}

impl From<steer_workspace::WorkspaceError> for Error {
    fn from(err: steer_workspace::WorkspaceError) -> Self {
        Error::Workspace(err.into())
    }
}
