use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::Tool;
use steer_tools::result::BashResult;
use steer_tools::tools::bash::BashParams;

use super::to_tools_context;

pub const BASH_TOOL_NAME: &str = "bash";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BashToolParams {
    pub command: String,
    #[schemars(range(min = 1, max = 3600000))]
    pub timeout: Option<u64>,
}

pub struct BashTool;

#[async_trait]
impl StaticTool for BashTool {
    type Params = BashToolParams;
    type Output = BashResult;

    const NAME: &'static str = BASH_TOOL_NAME;
    const DESCRIPTION: &'static str = "Run a bash command in the terminal";
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let bash_params = BashParams {
            command: params.command,
            timeout: params.timeout,
        };

        let params_json = serde_json::to_value(bash_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::BashTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result)
    }
}
