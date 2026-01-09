use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct WorkspaceOpContext {
    pub op_id: String,
    pub cancellation_token: CancellationToken,
}

impl WorkspaceOpContext {
    pub fn new(op_id: impl Into<String>, cancellation_token: CancellationToken) -> Self {
        Self {
            op_id: op_id.into(),
            cancellation_token,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileRequest {
    pub file_path: String,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListDirectoryRequest {
    pub path: String,
    pub ignore: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobRequest {
    pub pattern: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepRequest {
    pub pattern: String,
    pub include: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstGrepRequest {
    pub pattern: String,
    pub lang: Option<String>,
    pub include: Option<String>,
    pub exclude: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditOperation {
    pub old_string: String,
    pub new_string: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyEditsRequest {
    pub file_path: String,
    pub edits: Vec<EditOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileRequest {
    pub file_path: String,
    pub content: String,
}
