use async_trait::async_trait;

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::result::GlobResult;
use steer_tools::tools::glob::{GlobError, GlobParams, GlobToolSpec};
use super::workspace_op_error;
use steer_workspace::{GlobRequest, WorkspaceOpContext};

pub struct GlobTool;

#[async_trait]
impl StaticTool for GlobTool {
    type Params = GlobParams;
    type Output = GlobResult;
    type Spec = GlobToolSpec;

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
    ) -> Result<Self::Output, StaticToolError<GlobError>> {
        let request = GlobRequest {
            pattern: params.pattern,
            path: params.path,
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        ctx.services
            .workspace
            .glob(request, &op_ctx)
            .await
            .map_err(|e| StaticToolError::execution(GlobError::Workspace(workspace_op_error(e))))
    }
}
