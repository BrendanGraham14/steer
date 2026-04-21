use async_trait::async_trait;

use super::workspace_op_error;
use crate::tools::builtin_tool::{BuiltinTool, BuiltinToolContext, BuiltinToolError};
use crate::tools::capability::Capabilities;
use steer_tools::result::WcResult;
use steer_tools::tools::wc::{WcError, WcParams, WcToolSpec};
use steer_workspace::{WcRequest, WorkspaceOpContext};

pub struct WcTool;

#[async_trait]
impl BuiltinTool for WcTool {
    type Params = WcParams;
    type Output = WcResult;
    type Spec = WcToolSpec;

    const DESCRIPTION: &'static str = concat!(
        "Counts lines, words, and bytes in a single file using POSIX-style rules: ",
        "lines are newline (LF) counts, words are maximal runs of non-ASCII-whitespace bytes, ",
        "and bytes come from file size. Use this instead of loading large files into context ",
        "when you only need size statistics."
    );
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &BuiltinToolContext,
    ) -> Result<Self::Output, BuiltinToolError<WcError>> {
        let request = WcRequest {
            file_path: params.file_path,
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        ctx.services
            .workspace
            .wc(request, &op_ctx)
            .await
            .map_err(|e| BuiltinToolError::execution(WcError::Workspace(workspace_op_error(e))))
    }
}
