use async_trait::async_trait;

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::error::ToolExecutionError;
use steer_tools::result::GrepResult;
use steer_tools::tools::GREP_TOOL_NAME;
use steer_tools::tools::grep::GrepError;
use steer_tools::tools::grep::GrepParams;
use super::workspace_op_error;
use steer_workspace::{GrepRequest, WorkspaceOpContext};

pub struct GrepTool;

#[async_trait]
impl StaticTool for GrepTool {
    type Params = GrepParams;
    type Output = GrepResult;

    const NAME: &'static str = GREP_TOOL_NAME;
    const DESCRIPTION: &'static str = r#"Fast content search built on ripgrep for blazing performance at any scale.
- Searches using regular expressions or literal strings
- Supports regex syntax like "log.*Error", "function\\s+\\w+", etc.
- If the pattern isn't valid regex, it automatically searches for the literal text
- Filter files by name pattern with include parameter (e.g., "*.js", "*.{ts,tsx}")
- Automatically respects .gitignore files
- Returns matches as "filepath:line_number: line_content""#;
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let request = GrepRequest {
            pattern: params.pattern,
            include: params.include,
            path: params.path,
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        let result = ctx
            .services
            .workspace
            .grep(request, &op_ctx)
            .await
            .map_err(|e| {
                StaticToolError::execution(ToolExecutionError::Grep(GrepError::Workspace(
                    workspace_op_error(e),
                )))
            })?;
        Ok(GrepResult(result))
    }
}
