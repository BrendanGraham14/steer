use async_trait::async_trait;

use super::workspace_op_error;
use crate::tools::builtin_tool::{BuiltinTool, BuiltinToolContext, BuiltinToolError};
use crate::tools::capability::Capabilities;
use steer_tools::result::ReplaceResult;
use steer_tools::tools::replace::{ReplaceError, ReplaceParams, ReplaceToolSpec};
use steer_workspace::{WorkspaceOpContext, WriteFileRequest};

pub struct ReplaceTool;

#[async_trait]
impl BuiltinTool for ReplaceTool {
    type Params = ReplaceParams;
    type Output = ReplaceResult;
    type Spec = ReplaceToolSpec;

    const DESCRIPTION: &'static str = r"Writes a file to the local filesystem.

Before using this tool:

1. Use the read_file tool to understand the file's contents and context

2. Directory Verification (only applicable when creating new files):
 - Use the ls tool to verify the parent directory exists and is the correct location";
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &BuiltinToolContext,
    ) -> Result<Self::Output, BuiltinToolError<ReplaceError>> {
        let request = WriteFileRequest {
            file_path: params.file_path,
            content: params.content,
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        let result = ctx
            .services
            .workspace
            .write_file(request, &op_ctx)
            .await
            .map_err(|e| {
                BuiltinToolError::execution(ReplaceError::Workspace(workspace_op_error(e)))
            })?;
        Ok(ReplaceResult(result))
    }
}
