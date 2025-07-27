use serde::{Deserialize, Serialize};

/// Configuration for a workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceConfig {
    /// Local filesystem workspace
    Local {
        /// Path to the workspace directory
        path: std::path::PathBuf,
    },
    /// Remote workspace accessed via gRPC
    Remote {
        /// Address of the remote workspace service (e.g., "localhost:50051")
        address: String,
        /// Optional authentication for the remote service
        auth: Option<RemoteAuth>,
    },
}

/// Authentication information for remote workspaces
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RemoteAuth {
    /// Bearer token authentication
    BearerToken(String),
    /// API key authentication
    ApiKey(String),
}
