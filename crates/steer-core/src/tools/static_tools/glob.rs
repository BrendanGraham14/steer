
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::Tool;
use steer_tools::result::GlobResult;
use steer_tools::tools::glob::GlobParams;

use super::to_tools_context;

pub const GLOB_TOOL_NAME: &str = "glob";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GlobToolParams {
    pub pattern: String,
    pub path: Option<String>,
}

pub struct GlobTool;

#[async_trait]
impl StaticTool for GlobTool {
    type Params = GlobToolParams;
    type Output = GlobResult;

    const NAME: &'static str = GLOB_TOOL_NAME;
    const DESCRIPTION: &'static str = r#"Fast file pattern matching tool that works with any codebase size.
- Supports glob patterns like "**/*.js" or "src/**/*.ts"
- Returns matching file paths sorted by modification time
- Use this tool when you need to find files by name patterns"#;
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let glob_params = GlobParams {
            pattern: params.pattern,
            path: params.path,
        };

        let params_json = serde_json::to_value(glob_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::GlobTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result)
    }
}
