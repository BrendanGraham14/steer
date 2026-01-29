use std::fmt;
use thiserror::Error;

#[derive(Debug)]
pub struct GrpcStatus(Box<tonic::Status>);

impl GrpcStatus {
    pub fn code(&self) -> tonic::Code {
        self.0.code()
    }

    pub fn message(&self) -> &str {
        self.0.message()
    }
}

impl fmt::Display for GrpcStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "status: {:?}, message: \"{}\"",
            self.code(),
            self.message()
        )
    }
}

impl From<Box<tonic::Status>> for GrpcStatus {
    fn from(status: Box<tonic::Status>) -> Self {
        Self(status)
    }
}

impl From<tonic::Status> for GrpcStatus {
    fn from(status: tonic::Status) -> Self {
        Self(Box::new(status))
    }
}

impl From<GrpcStatus> for tonic::Status {
    fn from(status: GrpcStatus) -> Self {
        *status.0
    }
}

#[derive(Error, Debug)]
pub enum GrpcError {
    #[error("Failed to connect to gRPC server: {0}")]
    ConnectionFailed(#[from] tonic::transport::Error),

    #[error("gRPC call failed: {0}")]
    CallFailed(GrpcStatus),

    #[error("Failed to convert message at index {index}: {reason}")]
    MessageConversionFailed { index: usize, reason: String },

    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("Invalid session state: {reason}")]
    InvalidSessionState { reason: String },

    #[error("Stream error: {0}")]
    StreamError(String),

    #[error("Conversion error: {0}")]
    ConversionError(#[from] ConversionError),

    #[error("Core error: {0}")]
    CoreError(#[from] steer_core::error::Error),

    #[error("Channel receive error: {0}")]
    ChannelError(String),
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

    #[error("Invalid value '{value}' for field '{field}'")]
    InvalidValue { field: String, value: String },

    #[error("Invalid JSON for field '{field}': {error}")]
    InvalidJson { field: String, error: String },

    #[error("Invalid data: {message}")]
    InvalidData { message: String },

    #[error("Tool result conversion error: {0}")]
    ToolResultConversion(String),
}

impl From<GrpcError> for tonic::Status {
    fn from(err: GrpcError) -> Self {
        match err {
            GrpcError::ConnectionFailed(e) => {
                tonic::Status::unavailable(format!("Connection failed: {e}"))
            }
            GrpcError::CallFailed(status) => status.into(),
            GrpcError::MessageConversionFailed { index, reason } => {
                tonic::Status::invalid_argument(format!(
                    "Failed to convert message at index {index}: {reason}"
                ))
            }
            GrpcError::SessionNotFound { session_id } => {
                tonic::Status::not_found(format!("Session not found: {session_id}"))
            }
            GrpcError::InvalidSessionState { reason } => {
                tonic::Status::failed_precondition(format!("Invalid session state: {reason}"))
            }
            GrpcError::StreamError(msg) => tonic::Status::internal(format!("Stream error: {msg}")),
            GrpcError::ConversionError(e) => {
                tonic::Status::invalid_argument(format!("Conversion error: {e}"))
            }
            GrpcError::CoreError(e) => tonic::Status::internal(format!("Core error: {e}")),
            GrpcError::ChannelError(msg) => {
                tonic::Status::internal(format!("Channel error: {msg}"))
            }
        }
    }
}

impl From<Box<tonic::Status>> for GrpcError {
    fn from(status: Box<tonic::Status>) -> Self {
        GrpcError::CallFailed(GrpcStatus::from(status))
    }
}

impl From<tonic::Status> for GrpcError {
    fn from(status: tonic::Status) -> Self {
        GrpcError::CallFailed(GrpcStatus::from(status))
    }
}
