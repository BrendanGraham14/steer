use thiserror::Error;

use crate::api::ProviderKind;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Invalid authorization code")]
    InvalidCode,

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Token expired")]
    Expired,

    #[error("Re-authentication required")]
    ReauthRequired,

    #[error("OAuth flow cancelled by user")]
    Cancelled,

    #[error("Failed to start callback server: {0}")]
    CallbackServer(String),

    #[error("OAuth state mismatch")]
    StateMismatch,

    #[error("Invalid OAuth response: {0}")]
    InvalidResponse(String),

    #[error("Keyring error: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Unsupported authentication method: {method} for provider {provider}")]
    UnsupportedMethod {
        method: String,
        provider: ProviderKind,
    },

    #[error("Invalid authentication state: {0}")]
    InvalidState(String),

    #[error("Invalid credential: {0}")]
    InvalidCredential(String),

    #[error("Missing required input: {0}")]
    MissingInput(String),
}

pub type Result<T> = std::result::Result<T, AuthError>;
