use eventsource_stream::EventStreamError;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SseParseError {
    #[error("UTF-8 error: {details}")]
    Utf8 { details: String },
    #[error("Parse error: {details}")]
    Parser { details: String },
    #[error("Transport error: {details}")]
    Transport { details: String },
}

impl<E> From<EventStreamError<E>> for SseParseError
where
    E: std::error::Error,
{
    fn from(err: EventStreamError<E>) -> Self {
        match err {
            EventStreamError::Utf8(err) => Self::Utf8 {
                details: err.to_string(),
            },
            EventStreamError::Parser(err) => Self::Parser {
                details: err.to_string(),
            },
            EventStreamError::Transport(err) => Self::Transport {
                details: err.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStreamErrorKind {
    StreamError,
    StreamRetry,
    RateLimitExceeded,
    ResponseFailed,
    Overloaded,
    ServiceUnavailable,
    Timeout,
    Unknown(String),
}

impl ProviderStreamErrorKind {
    pub fn from_provider_error_type(error_type: &str) -> Self {
        match error_type {
            "stream_error" | "error" => Self::StreamError,
            "stream_retry" => Self::StreamRetry,
            "rate_limit_exceeded" | "rate_limit_error" => Self::RateLimitExceeded,
            "response_failed" => Self::ResponseFailed,
            "overloaded_error" => Self::Overloaded,
            "service_unavailable_error" => Self::ServiceUnavailable,
            "timeout_error" => Self::Timeout,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::StreamError
                | Self::StreamRetry
                | Self::RateLimitExceeded
                | Self::ResponseFailed
                | Self::Overloaded
                | Self::ServiceUnavailable
                | Self::Timeout
        )
    }
}

impl std::fmt::Display for ProviderStreamErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StreamError => f.write_str("stream_error"),
            Self::StreamRetry => f.write_str("stream_retry"),
            Self::RateLimitExceeded => f.write_str("rate_limit_exceeded"),
            Self::ResponseFailed => f.write_str("response_failed"),
            Self::Overloaded => f.write_str("overloaded_error"),
            Self::ServiceUnavailable => f.write_str("service_unavailable_error"),
            Self::Timeout => f.write_str("timeout_error"),
            Self::Unknown(raw) => f.write_str(raw),
        }
    }
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum StreamError {
    #[error("Request cancelled")]
    Cancelled,

    #[error("SSE parse error: {0}")]
    SseParse(SseParseError),

    #[error("{provider} error ({kind}): {message}")]
    Provider {
        provider: String,
        kind: ProviderStreamErrorKind,
        raw_error_type: Option<String>,
        message: String,
    },
}

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Authentication failed: {details}")]
    AuthenticationFailed { provider: String, details: String },

    #[error("Auth error: {0}")]
    AuthError(String),

    #[error("Rate limited by {provider}: {details}")]
    RateLimited { provider: String, details: String },

    #[error("Invalid request to {provider}: {details}")]
    InvalidRequest { provider: String, details: String },

    #[error("{provider} server error (Status: {status_code}): {details}")]
    ServerError {
        provider: String,
        status_code: u16,
        details: String,
    },

    #[error("Request timed out for {provider}")]
    Timeout { provider: String },

    #[error("Request cancelled for {provider}")]
    Cancelled { provider: String },

    #[error("Failed to parse response from {provider}: {details}")]
    ResponseParsingError { provider: String, details: String },

    #[error("API returned no choices/candidates for {provider}")]
    NoChoices { provider: String },

    #[error("Request blocked by {provider}: {details}")]
    RequestBlocked { provider: String, details: String },

    #[error("Unknown API error from {provider}: {details}")]
    Unknown { provider: String, details: String },

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("{provider} does not support {feature}: {details}")]
    UnsupportedFeature {
        provider: String,
        feature: String,
        details: String,
    },

    #[error("Stream error from {provider}: {details}")]
    StreamError { provider: String, details: String },
}

impl From<crate::error::Error> for ApiError {
    fn from(err: crate::error::Error) -> Self {
        match err {
            crate::error::Error::Api(api_err) => api_err,
            crate::error::Error::Configuration(msg) => ApiError::Configuration(msg),
            other => ApiError::Unknown {
                provider: "internal".to_string(),
                details: format!("Unexpected internal error: {other}"),
            },
        }
    }
}
