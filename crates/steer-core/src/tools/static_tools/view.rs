use async_trait::async_trait;

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::result::FileContentResult;
use steer_tools::tools::VIEW_TOOL_NAME;
use steer_tools::tools::view::ViewParams;
use steer_workspace::{ReadFileRequest, WorkspaceOpContext};

pub struct ViewTool;

#[async_trait]
impl StaticTool for ViewTool {
    type Params = ViewParams;
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
