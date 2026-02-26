#[cfg(feature = "schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub fn non_unique_match_preview_suffix(
    match_previews: &[EditMatchPreview],
    omitted_matches: usize,
) -> String {
    if match_previews.is_empty() {
        return String::new();
    }

    let mut parts = match_previews
        .iter()
        .map(|preview| {
            format!(
                "line {}:{} `{}`",
                preview.line_number, preview.column_number, preview.snippet
            )
        })
        .collect::<Vec<_>>();

    if omitted_matches > 0 {
        parts.push(format!("and {omitted_matches} more"));
    }

    format!("; matches: {}", parts.join("; "))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct EditMatchPreview {
    pub line_number: usize,
    pub column_number: usize,
    pub snippet: String,
}

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum EditFailure {
    #[error("file not found: {file_path}")]
    FileNotFound { file_path: String },

    #[error(
        "edit #{edit_index} has an empty old_string; use write_file to create or overwrite files"
    )]
    EmptyOldString { edit_index: usize },

    #[error("string not found for edit #{edit_index} in file {file_path}")]
    StringNotFound {
        file_path: String,
        edit_index: usize,
    },

    #[error(
        "invalid match selection for edit #{edit_index} in file {file_path}: {message}"
    )]
    InvalidMatchSelection {
        file_path: String,
        edit_index: usize,
        message: String,
    },

    #[error(
        "found {occurrences} matches for edit #{edit_index} in file {file_path}; old_string must match exactly once{preview_suffix}",
        preview_suffix = non_unique_match_preview_suffix(match_previews, *omitted_matches)
    )]
    NonUniqueMatch {
        file_path: String,
        edit_index: usize,
        occurrences: usize,
        #[serde(default)]
        match_previews: Vec<EditMatchPreview>,
        #[serde(default)]
        omitted_matches: usize,
    },
}

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceError {
    #[error("I/O error: {0}")]
    Io(String),

    #[error("Tool execution failed: {0}")]
    ToolExecution(String),

    #[error("Edit failed: {0}")]
    Edit(EditFailure),

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Status error: {0}")]
    Status(String),

    #[error("Not supported: {0}")]
    NotSupported(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    #[error("Remote workspace error: {0}")]
    Remote(String),
}

pub type Result<T> = std::result::Result<T, WorkspaceError>;

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum EnvironmentManagerError {
    #[error("Environment not found: {0}")]
    NotFound(String),

    #[error("Environment operation not supported: {0}")]
    NotSupported(String),

    #[error("Invalid environment request: {0}")]
    InvalidRequest(String),

    #[error("I/O error: {0}")]
    Io(String),

    #[error("Environment manager error: {0}")]
    Other(String),
}

pub type EnvironmentManagerResult<T> = std::result::Result<T, EnvironmentManagerError>;

impl From<std::io::Error> for EnvironmentManagerError {
    fn from(err: std::io::Error) -> Self {
        EnvironmentManagerError::Io(err.to_string())
    }
}

impl From<WorkspaceError> for EnvironmentManagerError {
    fn from(err: WorkspaceError) -> Self {
        match err {
            WorkspaceError::Io(message) => EnvironmentManagerError::Io(message),
            other => EnvironmentManagerError::Other(other.to_string()),
        }
    }
}

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceManagerError {
    #[error("Workspace not found: {0}")]
    NotFound(String),

    #[error("Workspace operation not supported: {0}")]
    NotSupported(String),

    #[error("Invalid workspace request: {0}")]
    InvalidRequest(String),

    #[error("I/O error: {0}")]
    Io(String),

    #[error("Workspace manager error: {0}")]
    Other(String),
}

pub type WorkspaceManagerResult<T> = std::result::Result<T, WorkspaceManagerError>;

impl From<std::io::Error> for WorkspaceManagerError {
    fn from(err: std::io::Error) -> Self {
        WorkspaceManagerError::Io(err.to_string())
    }
}

impl From<WorkspaceError> for WorkspaceManagerError {
    fn from(err: WorkspaceError) -> Self {
        match err {
            WorkspaceError::Io(message) => WorkspaceManagerError::Io(message),
            other => WorkspaceManagerError::Other(other.to_string()),
        }
    }
}

impl From<tonic::transport::Error> for WorkspaceError {
    fn from(err: tonic::transport::Error) -> Self {
        WorkspaceError::Transport(err.to_string())
    }
}

impl From<tonic::Status> for WorkspaceError {
    fn from(err: tonic::Status) -> Self {
        WorkspaceError::Status(err.to_string())
    }
}

impl From<std::io::Error> for WorkspaceError {
    fn from(err: std::io::Error) -> Self {
        WorkspaceError::Io(err.to_string())
    }
}

impl From<EditFailure> for WorkspaceError {
    fn from(err: EditFailure) -> Self {
        WorkspaceError::Edit(err)
    }
}
