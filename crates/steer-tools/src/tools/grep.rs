use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::{ToolExecutionError, WorkspaceOpError};
use crate::result::GrepResult;

pub const GREP_TOOL_NAME: &str = "grep";

pub struct GrepToolSpec;

impl ToolSpec for GrepToolSpec {
    type Params = GrepParams;
    type Result = GrepResult;
    type Error = GrepError;

    const NAME: &'static str = GREP_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Grep";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::Grep(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum GrepError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GrepParams {
    /// The search pattern (regex or literal string). If invalid regex, searches for literal text
    #[schemars(length(min = 1))]
    pub pattern: String,
    /// Optional glob pattern to filter files by name (e.g., "*.rs", "*.{ts,tsx}")
    pub include: Option<String>,
    /// Optional directory to search in (defaults to current working directory)
    pub path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::GrepParams;
    use crate::InputSchema;
    use schemars::schema_for;
    use serde_json::json;

    #[test]
    fn grep_pattern_has_min_length_constraint() {
        let input_schema: InputSchema = schema_for!(GrepParams).into();
        let pattern_schema = input_schema
            .as_value()
            .get("properties")
            .and_then(|properties| properties.get("pattern"))
            .expect("grep pattern schema should exist");

        assert_eq!(pattern_schema.get("minLength"), Some(&json!(1)));
    }
}
