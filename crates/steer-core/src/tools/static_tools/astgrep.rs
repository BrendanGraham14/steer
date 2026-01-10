use async_trait::async_trait;

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::result::AstGrepResult;
use steer_tools::tools::AST_GREP_TOOL_NAME;
use steer_tools::tools::astgrep::AstGrepParams;
use steer_workspace::{AstGrepRequest, WorkspaceOpContext};

pub struct AstGrepTool;

#[async_trait]
impl StaticTool for AstGrepTool {
    type Params = AstGrepParams;
    type Output = AstGrepResult;

    const NAME: &'static str = AST_GREP_TOOL_NAME;
    const DESCRIPTION: &'static str = r#"Structural code search using abstract syntax trees (AST).
- Searches code by its syntactic structure, not just text patterns
- Use $METAVAR placeholders (e.g., $VAR, $FUNC, $ARGS) to match any code element
- Supports all major languages: rust, javascript, typescript, python, java, go, etc.
Pattern examples:
- "console.log($MSG)" - finds all console.log calls regardless of argument
- "fn $NAME($PARAMS) { $BODY }" - finds all Rust function definitions
- "if $COND { $THEN } else { $ELSE }" - finds all if-else statements
- "import $WHAT from '$MODULE'" - finds all ES6 imports from specific modules
- "$VAR = $VAR + $EXPR" - finds all self-incrementing assignments
Advanced patterns:
- "function $FUNC($$$ARGS) { $$$ }" - $$$ matches any number of elements
- "foo($ARG, ...)" - ellipsis matches remaining arguments
- Use any valid code as a pattern - ast-grep understands the syntax!
Automatically respects .gitignore files"#;
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let request = AstGrepRequest {
            pattern: params.pattern,
            lang: params.lang,
            include: params.include,
            exclude: params.exclude,
            path: params.path,
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        let result = ctx
            .services
            .workspace
            .astgrep(request, &op_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;
        Ok(AstGrepResult(result))
    }
}
