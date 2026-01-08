use steer_workspace::WorkspaceError;
use thiserror::Error;

use crate::{api::ApiError, auth::AuthError, tools::ToolError};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Tool(#[from] ToolError),
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
    #[error("Bash command error: {0}")]
    BashCommandError(String),
}

impl From<steer_tools::ToolError> for Error {
    fn from(err: steer_tools::ToolError) -> Self {
        Error::Tool(err.into())
    }
}
