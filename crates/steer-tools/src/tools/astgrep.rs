use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const AST_GREP_TOOL_NAME: &str = "astgrep";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AstGrepParams {
    /// The search pattern (code pattern with $METAVAR placeholders)
    pub pattern: String,
    /// Language (rust, tsx, python, etc.)
    pub lang: Option<String>,
    /// Optional glob pattern to filter files by name (e.g., "*.rs", "*.{ts,tsx}")
    pub include: Option<String>,
    /// Optional glob pattern to exclude files
    pub exclude: Option<String>,
    /// Optional directory to search in (defaults to current working directory)
    pub path: Option<String>,
}
