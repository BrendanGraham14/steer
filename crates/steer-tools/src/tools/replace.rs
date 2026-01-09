use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const REPLACE_TOOL_NAME: &str = "write_file";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReplaceParams {
    /// The absolute path to the file to write (must be absolute, not relative)
    pub file_path: String,
    /// The content to write to the file
    pub content: String,
}
