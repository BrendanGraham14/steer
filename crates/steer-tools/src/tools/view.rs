use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const VIEW_TOOL_NAME: &str = "read_file";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ViewParams {
    /// The absolute path to the file to read
    pub file_path: String,
    /// The line number to start reading from (1-indexed)
    pub offset: Option<u64>,
    /// The maximum number of lines to read
    pub limit: Option<u64>,
}
