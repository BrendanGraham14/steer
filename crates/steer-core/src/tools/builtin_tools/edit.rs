use super::workspace_op_error;
use crate::tools::builtin_tool::{BuiltinTool, BuiltinToolContext, BuiltinToolError};
use crate::tools::capability::Capabilities;
use async_trait::async_trait;
use steer_tools::result::{EditResult, MultiEditResult};
use steer_tools::tools::edit::multi_edit::{MultiEditError, MultiEditParams, MultiEditToolSpec};
use steer_tools::tools::edit::{EditError, EditFailure, EditParams, EditToolSpec};
use steer_workspace::{ApplyEditsRequest, EditOperation, WorkspaceOpContext};

pub struct EditTool;

const EDIT_DESCRIPTION: &str = r"This is a tool for editing files. For moving or renaming files, you should generally use the Bash tool with the 'mv' command instead. For larger edits, use the replace tool to overwrite files.

Before using this tool:

1. Use the View tool to understand the file's contents and context

2. Verify the directory path is correct:
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
 - old_string must be non-empty; empty old_string is rejected

If you want to create a new file or overwrite an entire file, use the dedicated file-write tool instead.

Remember: when making multiple file edits in a row to the same file, you should prefer to send all edits in a single message with multiple calls to this tool, rather than multiple messages with a single call each.";

#[async_trait]
impl BuiltinTool for EditTool {
    type Params = EditParams;
    type Output = EditResult;
    type Spec = EditToolSpec;

    const DESCRIPTION: &'static str = EDIT_DESCRIPTION;
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &BuiltinToolContext,
    ) -> Result<Self::Output, BuiltinToolError<EditError>> {
        let request = ApplyEditsRequest {
            file_path: params.file_path,
            edits: vec![EditOperation {
                old_string: params.old_string,
                new_string: params.new_string,
            }],
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        ctx.services
            .workspace
            .apply_edits(request, &op_ctx)
            .await
            .map_err(|e| BuiltinToolError::execution(map_workspace_edit_error(e)))
    }
}

pub struct MultiEditTool;

#[async_trait]
impl BuiltinTool for MultiEditTool {
    type Params = MultiEditParams;
    type Output = MultiEditResult;
    type Spec = MultiEditToolSpec;

    const DESCRIPTION: &'static str = "This is a tool for making multiple edits to a single file in one operation. Prefer this tool over the edit_file tool when you need to make multiple edits to the same file. Edits are applied sequentially in the provided order, and each old_string must match exactly one location against the latest file content after prior edits.";
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &BuiltinToolContext,
    ) -> Result<Self::Output, BuiltinToolError<MultiEditError>> {
        let request = ApplyEditsRequest {
            file_path: params.file_path,
            edits: params
                .edits
                .into_iter()
                .map(|e| EditOperation {
                    old_string: e.old_string,
                    new_string: e.new_string,
                })
                .collect(),
        };
        let op_ctx =
            WorkspaceOpContext::new(ctx.tool_call_id.0.clone(), ctx.cancellation_token.clone());
        let result = ctx
            .services
            .workspace
            .apply_edits(request, &op_ctx)
            .await
            .map_err(|e| BuiltinToolError::execution(map_workspace_multi_edit_error(e)))?;
        Ok(MultiEditResult(result))
    }
}

fn map_workspace_edit_error(err: steer_workspace::WorkspaceError) -> EditError {
    match err {
        steer_workspace::WorkspaceError::Edit(edit_failure) => {
            EditError::EditFailure(map_edit_failure(edit_failure))
        }
        other => EditError::Workspace(workspace_op_error(other)),
    }
}

fn map_workspace_multi_edit_error(err: steer_workspace::WorkspaceError) -> MultiEditError {
    match err {
        steer_workspace::WorkspaceError::Edit(edit_failure) => {
            MultiEditError::EditFailure(map_edit_failure(edit_failure))
        }
        other => MultiEditError::Workspace(workspace_op_error(other)),
    }
}

fn map_edit_failure(failure: steer_workspace::error::EditFailure) -> EditFailure {
    match failure {
        steer_workspace::error::EditFailure::FileNotFound { file_path } => {
            EditFailure::FileNotFound { file_path }
        }
        steer_workspace::error::EditFailure::EmptyOldString { edit_index } => {
            EditFailure::EmptyOldString { edit_index }
        }
        steer_workspace::error::EditFailure::StringNotFound {
            file_path,
            edit_index,
        } => EditFailure::StringNotFound {
            file_path,
            edit_index,
        },
        steer_workspace::error::EditFailure::NonUniqueMatch {
            file_path,
            edit_index,
            occurrences,
        } => EditFailure::NonUniqueMatch {
            file_path,
            edit_index,
            occurrences,
        },
    }
}
