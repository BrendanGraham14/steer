
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::Tool;
use super::workspace_op_error;
use steer_tools::result::FileListResult;
use steer_tools::tools::ls::LsParams;

use super::to_tools_context;

pub const LS_TOOL_NAME: &str = "ls";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LsToolParams {
    pub path: String,
    pub ignore: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct LsToolOutput {
    pub entries: Vec<FileEntry>,
    pub base_path: String,
}

#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub permissions: Option<String>,
}

impl From<FileListResult> for LsToolOutput {
    fn from(r: FileListResult) -> Self {
        Self {
            entries: r
                .entries
                .into_iter()
                .map(|e| FileEntry {
                    path: e.path,
                    is_directory: e.is_directory,
                    size: e.size,
                    permissions: e.permissions,
                })
                .collect(),
            base_path: r.base_path,
        }
    }
}

pub struct LsTool;

#[async_trait]
impl StaticTool for LsTool {
    type Params = LsToolParams;
    type Output = LsToolOutput;

    const NAME: &'static str = LS_TOOL_NAME;
    const DESCRIPTION: &'static str = "Lists files and directories in a given path. The path parameter must be an absolute path, not a relative path. You should generally prefer the Glob and Grep tools, if you know which directories to search.";
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let ls_params = LsParams {
            path: params.path,
            ignore: params.ignore,
        };

        let params_json = serde_json::to_value(ls_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::LsTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result.into())
    }
}
