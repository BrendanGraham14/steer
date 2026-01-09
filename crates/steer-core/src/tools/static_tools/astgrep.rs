
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::Tool;
use steer_tools::result::AstGrepResult;
use steer_tools::tools::astgrep::AstGrepParams;

use super::to_tools_context;

pub const AST_GREP_TOOL_NAME: &str = "astgrep";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AstGrepToolParams {
    pub pattern: String,
    pub lang: Option<String>,
    pub include: Option<String>,
    pub exclude: Option<String>,
    pub path: Option<String>,
}

pub struct AstGrepTool;

#[async_trait]
impl StaticTool for AstGrepTool {
    type Params = AstGrepToolParams;
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
        let tools_ctx = to_tools_context(ctx);

        let astgrep_params = AstGrepParams {
            pattern: params.pattern,
            lang: params.lang,
            include: params.include,
            exclude: params.exclude,
            path: params.path,
        };

        let params_json = serde_json::to_value(astgrep_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::AstGrepTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result)
    }
}
