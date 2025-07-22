//! Error types for the steer-tui crate

use std::io;
use thiserror::Error;

/// Result type alias for steer-tui operations
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for steer-tui
#[derive(Error, Debug)]
pub enum Error {
    /// Terminal I/O errors
    #[error("Terminal I/O error: {0}")]
    Io(#[from] io::Error),

    /// Event processing errors
    #[error("Event processing error: {0}")]
    EventProcessing(String),

    /// UI rendering errors
    #[error("UI rendering error: {0}")]
    Rendering(String),

    /// Channel communication errors
    #[error("Channel error: {0}")]
    Channel(String),

    /// Invalid state errors
    #[error("Invalid UI state: {0}")]
    InvalidState(String),

    /// Model selection errors
    #[error("Model selection error: {0}")]
    ModelSelection(String),

    /// Notification errors
    #[error("Notification error: {0}")]
    Notification(String),

    /// Command processing errors
    #[error("Command processing error: {0}")]
    CommandProcessing(String),

    /// Timeout errors
    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// Core errors from steer-core
    #[error("Core error: {0}")]
    Core(#[from] steer_core::error::Error),

    /// Generic errors
    #[error("{0}")]
    Generic(String),

    /// Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    /// Authentication errors
    #[error("Authentication error: {0}")]
    Auth(String),

    /// gRPC errors from steer-grpc
    #[error("gRPC error: {0}")]
    Grpc(#[from] Box<steer_grpc::GrpcError>),

    /// TUI command parsing errors
    #[error("TUI command parsing error: {0}")]
    TuiCommandParsing(#[from] crate::tui::commands::TuiCommandError),
}

// Convert notify-rust errors to our error type
impl From<notify_rust::error::Error> for Error {
    fn from(err: notify_rust::error::Error) -> Self {
        Error::Notification(err.to_string())
    }
}
