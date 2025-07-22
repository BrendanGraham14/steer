use thiserror::Error;

#[derive(Error, Debug)]
pub enum GrpcError {
    #[error("Failed to connect to gRPC server: {0}")]
    ConnectionFailed(#[from] tonic::transport::Error),

    #[error("gRPC call failed: {0}")]
    CallFailed(#[from] Box<tonic::Status>),

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
            GrpcError::CallFailed(status) => *status,
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
