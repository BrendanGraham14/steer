
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::Tool;
use super::workspace_op_error;
use steer_tools::result::FileContentResult;
use steer_tools::tools::view::ViewParams;

use super::to_tools_context;

pub const VIEW_TOOL_NAME: &str = "read_file";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ViewToolParams {
    pub file_path: String,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ViewToolOutput {
    pub content: String,
    pub file_path: String,
    pub line_count: usize,
    pub truncated: bool,
}

impl From<FileContentResult> for ViewToolOutput {
    fn from(r: FileContentResult) -> Self {
        Self {
            content: r.content,
            file_path: r.file_path,
            line_count: r.line_count,
            truncated: r.truncated,
        }
    }
}

pub struct ViewTool;

#[async_trait]
impl StaticTool for ViewTool {
    type Params = ViewToolParams;
    type Output = ViewToolOutput;

    const NAME: &'static str = VIEW_TOOL_NAME;
    const DESCRIPTION: &'static str = concat!(
        "Reads a file from the local filesystem. The file_path parameter must be an absolute path, not a relative path.\n",
        "By default, it reads up to 2000 lines starting from the beginning of the file. You can optionally specify a line offset and limit\n",
        "(especially handy for long files), but it's recommended to read the whole file by not providing these parameters.\n",
        "Any lines longer than 2000 characters will be truncated."
    );
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let view_params = ViewParams {
            file_path: params.file_path,
            offset: params.offset,
            limit: params.limit,
        };

        let params_json = serde_json::to_value(view_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::ViewTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result.into())
    }
}
