use thiserror::Error;

#[derive(Error, Debug)]
pub enum GrpcError {
    #[error("Failed to connect to gRPC server: {0}")]
    ConnectionFailed(#[from] tonic::transport::Error),

    #[error("gRPC call failed: {0}")]
    CallFailed(#[from] tonic::Status),

    #[error("Failed to convert message at index {index}: {reason}")]
    MessageConversionFailed { index: usize, reason: String },

    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("Invalid session state: {reason}")]
    InvalidSessionState { reason: String },

    #[error("Stream error: {0}")]
    StreamError(String),
}