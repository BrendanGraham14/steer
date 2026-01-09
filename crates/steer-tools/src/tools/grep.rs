use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const GREP_TOOL_NAME: &str = "grep";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GrepParams {
    /// The search pattern (regex or literal string). If invalid regex, searches for literal text
    pub pattern: String,
    /// Optional glob pattern to filter files by name (e.g., "*.rs", "*.{ts,tsx}")
    pub include: Option<String>,
    /// Optional directory to search in (defaults to current working directory)
    pub path: Option<String>,
}
