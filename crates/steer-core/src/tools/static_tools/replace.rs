
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::Tool;
use steer_tools::result::ReplaceResult;
use steer_tools::tools::replace::ReplaceParams;

use super::to_tools_context;

pub const REPLACE_TOOL_NAME: &str = "write_file";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplaceToolParams {
    pub file_path: String,
    pub content: String,
}

pub struct ReplaceTool;

#[async_trait]
impl StaticTool for ReplaceTool {
    type Params = ReplaceToolParams;
    type Output = ReplaceResult;

    const NAME: &'static str = REPLACE_TOOL_NAME;
    const DESCRIPTION: &'static str = r#"Writes a file to the local filesystem.

Before using this tool:

1. Use the read_file tool to understand the file's contents and context

2. Directory Verification (only applicable when creating new files):
 - Use the ls tool to verify the parent directory exists and is the correct location"#;
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let replace_params = ReplaceParams {
            file_path: params.file_path,
            content: params.content,
        };

        let params_json = serde_json::to_value(replace_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::ReplaceTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result)
    }
}
