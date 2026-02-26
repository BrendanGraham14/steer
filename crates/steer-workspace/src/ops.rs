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
    pub raw: Option<bool>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EditMatchSelection {
    ExactlyOne,
    First,
    All,
    Nth { match_index: Option<u64> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditOperation {
    pub old_string: String,
    pub new_string: String,
    pub match_selection: Option<EditMatchSelection>,
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

#[cfg(test)]
mod tests {
    use super::EditMatchSelection;

    #[test]
    fn test_edit_match_selection_partial_eq() {
        assert_eq!(
            EditMatchSelection::ExactlyOne,
            EditMatchSelection::ExactlyOne
        );
        assert_eq!(EditMatchSelection::First, EditMatchSelection::First);
        assert_eq!(EditMatchSelection::All, EditMatchSelection::All);
        assert_eq!(
            EditMatchSelection::Nth {
                match_index: Some(2)
            },
            EditMatchSelection::Nth {
                match_index: Some(2)
            }
        );
        assert_ne!(
            EditMatchSelection::Nth {
                match_index: Some(1)
            },
            EditMatchSelection::Nth {
                match_index: Some(2)
            }
        );
    }
}
