use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to serialize preferences: {0}")]
    Serialization(#[from] toml::ser::Error),

    #[error("Process execution failed: {0}")]
    Process(String),

    #[error(transparent)]
    Core(#[from] steer_core::error::Error),
}
