use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Authentication failed: {details}")]
    AuthenticationFailed { provider: String, details: String },

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
