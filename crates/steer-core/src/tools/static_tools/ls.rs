use async_trait::async_trait;

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::result::FileListResult;
use steer_tools::tools::ls::{LsError, LsParams, LsToolSpec};
use super::workspace_op_error;
use steer_workspace::{ListDirectoryRequest, WorkspaceOpContext};

pub struct LsTool;

#[async_trait]
impl StaticTool for LsTool {
    type Params = LsParams;
    type Output = FileListResult;
    type Spec = LsToolSpec;

    const DESCRIPTION: &'static str = "Lists files and directories in a given path. The path parameter must be an absolute path, not a relative path. You should generally prefer the Glob and Grep tools, if you know which directories to search.";
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError<LsError>> {
        let request = ListDirectoryRequest {
            path: params.path,
            ignore: params.ignore,
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        ctx.services
            .workspace
            .list_directory(request, &op_ctx)
            .await
            .map_err(|e| {
                StaticToolError::execution(LsError::Workspace(workspace_op_error(e)))
            })
    }
}
