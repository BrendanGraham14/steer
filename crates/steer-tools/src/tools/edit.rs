use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const EDIT_TOOL_NAME: &str = "edit_file";

#[derive(Deserialize, Serialize, Debug, JsonSchema, Clone)]
pub struct SingleEditOperation {
    /// The exact string to find and replace.
    pub old_string: String,
    /// The string to replace `old_string` with.
    pub new_string: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EditParams {
    /// The absolute path to the file to edit
    pub file_path: String,
    /// The exact string to find and replace. If empty, the file will be created.
    pub old_string: String,
    /// The string to replace `old_string` with.
    pub new_string: String,
}

pub mod multi_edit {
    use super::*;

    pub const MULTI_EDIT_TOOL_NAME: &str = "multi_edit";

    #[derive(Deserialize, Serialize, Debug, JsonSchema)]
    pub struct MultiEditParams {
        /// The absolute path to the file to edit.
        pub file_path: String,
        /// A list of edit operations to apply sequentially.
        pub edits: Vec<SingleEditOperation>,
    }
}
