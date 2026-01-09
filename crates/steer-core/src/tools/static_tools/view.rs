
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::result::FileContentResult;
use steer_workspace::{ReadFileRequest, WorkspaceOpContext};

pub const VIEW_TOOL_NAME: &str = "read_file";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ViewToolParams {
    pub file_path: String,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

pub struct ViewTool;

#[async_trait]
impl StaticTool for ViewTool {
    type Params = ViewToolParams;
    type Output = FileContentResult;

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
        let request = ReadFileRequest {
            file_path: params.file_path,
            offset: params.offset,
            limit: params.limit,
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        ctx.services
            .workspace
            .read_file(request, &op_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))
    }
}
