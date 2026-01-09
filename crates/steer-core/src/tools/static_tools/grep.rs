
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::Tool;
use steer_tools::result::GrepResult;
use steer_tools::tools::grep::GrepParams;

use super::to_tools_context;

pub const GREP_TOOL_NAME: &str = "grep";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrepToolParams {
    pub pattern: String,
    pub include: Option<String>,
    pub path: Option<String>,
}

pub struct GrepTool;

#[async_trait]
impl StaticTool for GrepTool {
    type Params = GrepToolParams;
    type Output = GrepResult;

    const NAME: &'static str = GREP_TOOL_NAME;
    const DESCRIPTION: &'static str = r#"Fast content search built on ripgrep for blazing performance at any scale.
- Searches using regular expressions or literal strings
- Supports regex syntax like "log.*Error", "function\\s+\\w+", etc.
- If the pattern isn't valid regex, it automatically searches for the literal text
- Filter files by name pattern with include parameter (e.g., "*.js", "*.{ts,tsx}")
- Automatically respects .gitignore files
- Returns matches as "filepath:line_number: line_content""#;
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let grep_params = GrepParams {
            pattern: params.pattern,
            include: params.include,
            path: params.path,
        };

        let params_json = serde_json::to_value(grep_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::GrepTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result)
    }
}
