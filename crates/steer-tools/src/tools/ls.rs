use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const LS_TOOL_NAME: &str = "ls";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LsParams {
    /// The absolute path to the directory to list (must be absolute, not relative)
    pub path: String,
    /// Optional list of glob patterns to ignore
    pub ignore: Option<Vec<String>>,
}
