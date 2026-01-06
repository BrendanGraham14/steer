use super::workspace_op_error;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::Tool;
use steer_tools::result::EditResult;
use steer_tools::tools::edit::EditParams;
use steer_tools::tools::edit::multi_edit::MultiEditParams;

use super::to_tools_context;

pub const EDIT_TOOL_NAME: &str = "edit_file";
pub const MULTI_EDIT_TOOL_NAME: &str = "multi_edit_file";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditToolParams {
    pub file_path: String,
    pub old_string: String,
    pub new_string: String,
}

#[derive(Debug, Serialize)]
pub struct EditToolOutput {
    pub file_path: String,
    pub changes_made: usize,
    pub file_created: bool,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
}

impl From<EditResult> for EditToolOutput {
    fn from(r: EditResult) -> Self {
        Self {
            file_path: r.file_path,
            changes_made: r.changes_made,
            file_created: r.file_created,
            old_content: r.old_content,
            new_content: r.new_content,
        }
    }
}

pub struct EditTool;

const EDIT_DESCRIPTION: &str = r#"This is a tool for editing files. For moving or renaming files, you should generally use the Bash tool with the 'mv' command instead. For larger edits, use the replace tool to overwrite files.

Before using this tool:

1. Use the View tool to understand the file's contents and context

2. Verify the directory path is correct (only applicable when creating new files):
 - Use the LS tool to verify the parent directory exists and is the correct location

To make a file edit, provide the following:
1. file_path: The absolute path to the file to modify (must be absolute, not relative)
2. old_string: The text to replace (must be unique within the file, and must match the file contents exactly, including all whitespace and indentation)
3. new_string: The edited text to replace the old_string

The tool will replace ONE occurrence of old_string with new_string in the specified file.

CRITICAL REQUIREMENTS FOR USING THIS TOOL:

1. UNIQUENESS: The old_string MUST uniquely identify the specific instance you want to change. This means:
 - Include AT LEAST 3-5 lines of context BEFORE the change point
 - Include AT LEAST 3-5 lines of context AFTER the change point
 - Include all whitespace, indentation, and surrounding code exactly as it appears in the file

2. SINGLE INSTANCE: This tool can only change ONE instance at a time. If you need to change multiple instances:
 - Make separate calls to this tool for each instance
 - Each call must uniquely identify its specific instance using extensive context

3. VERIFICATION: Before using this tool:
 - Check how many instances of the target text exist in the file
 - If multiple instances exist, gather enough context to uniquely identify each one
 - Plan separate tool calls for each instance

WARNING: If you do not follow these requirements:
 - The tool will fail if old_string matches multiple locations
 - The tool will fail if old_string doesn't match exactly (including whitespace)
 - You may change the wrong instance if you don't include enough context

When making edits:
 - Ensure the edit results in idiomatic, correct code
 - Do not leave the code in a broken state
 - Always use absolute file paths (starting with /)

If you want to create a new file, use:
 - A new file path, including dir name if needed
 - An empty old_string
 - The new file's contents as new_string

Remember: when making multiple file edits in a row to the same file, you should prefer to send all edits in a single message with multiple calls to this tool, rather than multiple messages with a single call each."#;

#[async_trait]
impl StaticTool for EditTool {
    type Params = EditToolParams;
    type Output = EditToolOutput;

    const NAME: &'static str = EDIT_TOOL_NAME;
    const DESCRIPTION: &'static str = EDIT_DESCRIPTION;
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let edit_params = EditParams {
            file_path: params.file_path,
            old_string: params.old_string,
            new_string: params.new_string,
        };

        let params_json = serde_json::to_value(edit_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::EditTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result.into())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SingleEditOperation {
    pub old_string: String,
    pub new_string: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MultiEditToolParams {
    pub file_path: String,
    pub edits: Vec<SingleEditOperation>,
}

pub struct MultiEditTool;

#[async_trait]
impl StaticTool for MultiEditTool {
    type Params = MultiEditToolParams;
    type Output = EditToolOutput;

    const NAME: &'static str = MULTI_EDIT_TOOL_NAME;
    const DESCRIPTION: &'static str = "This is a tool for making multiple edits to a single file in one operation. Prefer this tool over the edit_file tool when you need to make multiple edits to the same file.";
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let multi_edit_params = MultiEditParams {
            file_path: params.file_path,
            edits: params
                .edits
                .into_iter()
                .map(|e| steer_tools::tools::edit::SingleEditOperation {
                    old_string: e.old_string,
                    new_string: e.new_string,
                })
                .collect(),
        };

        let params_json = serde_json::to_value(multi_edit_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::MultiEditTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result.0.into())
    }
}
