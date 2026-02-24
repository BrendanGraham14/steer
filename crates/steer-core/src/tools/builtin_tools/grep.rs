use async_trait::async_trait;
use std::time::Duration;
use tokio::time::timeout;

use super::workspace_op_error;
use crate::tools::builtin_tool::{BuiltinTool, BuiltinToolContext, BuiltinToolError};
use crate::tools::capability::Capabilities;
use steer_tools::result::GrepResult;
use steer_tools::tools::grep::{GrepError, GrepParams, GrepToolSpec};
use steer_workspace::{GrepRequest, WorkspaceOpContext};

pub struct GrepTool;

#[async_trait]
impl BuiltinTool for GrepTool {
    type Params = GrepParams;
    type Output = GrepResult;
    type Spec = GrepToolSpec;

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
        ctx: &BuiltinToolContext,
    ) -> Result<Self::Output, BuiltinToolError<GrepError>> {
        const GREP_TIMEOUT: Duration = Duration::from_secs(30);

        let request = validate_and_build_request(params)?;
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());

        tokio::select! {
            () = ctx.cancellation_token.cancelled() => Err(BuiltinToolError::Cancelled),
            result = timeout(GREP_TIMEOUT, ctx.services.workspace.grep(request, &op_ctx)) => {
                match result {
                    Ok(Ok(search_result)) => Ok(GrepResult(search_result)),
                    Ok(Err(error)) => Err(BuiltinToolError::execution(GrepError::Workspace(workspace_op_error(error)))),
                    Err(_) => Err(BuiltinToolError::Timeout),
                }
            }
        }
    }
}

fn validate_and_build_request(
    params: GrepParams,
) -> Result<GrepRequest, BuiltinToolError<GrepError>> {
    if params.pattern.is_empty() {
        return Err(BuiltinToolError::invalid_params(
            "pattern must be a non-empty string",
        ));
    }

    Ok(GrepRequest {
        pattern: params.pattern,
        include: params.include,
        path: params.path,
    })
}

#[cfg(test)]
mod tests {
    use super::validate_and_build_request;
    use steer_tools::tools::grep::GrepParams;

    #[test]
    fn empty_pattern_is_rejected() {
        let result = validate_and_build_request(GrepParams {
            pattern: String::new(),
            include: None,
            path: None,
        });

        let error = result.expect_err("empty pattern should fail validation");
        assert_eq!(
            error.to_string(),
            "Invalid parameters: pattern must be a non-empty string"
        );
    }

    #[test]
    fn non_empty_pattern_builds_request() {
        let request = validate_and_build_request(GrepParams {
            pattern: "needle".to_string(),
            include: Some("*.rs".to_string()),
            path: Some("src".to_string()),
        })
        .expect("non-empty pattern should pass validation");

        assert_eq!(request.pattern, "needle");
        assert_eq!(request.include.as_deref(), Some("*.rs"));
        assert_eq!(request.path.as_deref(), Some("src"));
    }
}
