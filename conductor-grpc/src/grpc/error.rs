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

#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("Missing required field: {field}")]
    MissingField { field: String },

    #[error("Invalid enum value: {value} for {enum_name}")]
    InvalidEnumValue { value: i32, enum_name: String },

    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Invalid variant: expected {expected}, got {actual}")]
    InvalidVariant { expected: String, actual: String },

    #[error("Missing oneof variant in {message}")]
    MissingOneofVariant { message: String },
}