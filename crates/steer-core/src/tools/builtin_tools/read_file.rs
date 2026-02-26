use async_trait::async_trait;

use super::workspace_op_error;
use crate::tools::builtin_tool::{BuiltinTool, BuiltinToolContext, BuiltinToolError};
use crate::tools::capability::Capabilities;
use steer_tools::result::FileContentResult;
use steer_tools::tools::read_file::{ReadFileError, ReadFileParams, ReadFileToolSpec};
use steer_workspace::{ReadFileRequest, WorkspaceOpContext};

pub struct ReadFileTool;

#[async_trait]
impl BuiltinTool for ReadFileTool {
    type Params = ReadFileParams;
    type Output = FileContentResult;
    type Spec = ReadFileToolSpec;

    const DESCRIPTION: &'static str = concat!(
        "Reads a file from the local filesystem. The file_path parameter must be an absolute path, not a relative path.\n",
        "By default, it reads up to 2000 lines starting from the beginning of the file. You can optionally specify a line offset and limit\n",
        "(especially handy for long files), but it's recommended to read the whole file by not providing these parameters.\n",
        "Any lines longer than 2000 characters will be truncated.\n",
        "Set raw=true to return unnumbered, untrimmed content without truncation for exact copy/paste."
    );
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &BuiltinToolContext,
    ) -> Result<Self::Output, BuiltinToolError<ReadFileError>> {
        let request = ReadFileRequest {
            file_path: params.file_path,
            offset: params.offset,
            limit: params.limit,
            raw: params.raw,
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        ctx.services
            .workspace
            .read_file(request, &op_ctx)
            .await
            .map_err(|e| {
                BuiltinToolError::execution(ReadFileError::Workspace(workspace_op_error(e)))
            })
    }
}
