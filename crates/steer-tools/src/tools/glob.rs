use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const GLOB_TOOL_NAME: &str = "glob";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GlobParams {
    /// The glob pattern to match files against
    pub pattern: String,
    /// Optional directory to search in. Defaults to the current working directory.
    pub path: Option<String>,
}
