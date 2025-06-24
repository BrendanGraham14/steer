use conductor_macros::tool;
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::context::ExecutionContext;
use crate::error::ToolError;

// Global lock manager for file paths
static FILE_LOCKS: Lazy<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

async fn get_file_lock(file_path: &str) -> Arc<Mutex<()>> {
    let mut locks_map_guard = FILE_LOCKS.lock().await;
    locks_map_guard
        .entry(file_path.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[derive(Deserialize, Debug, JsonSchema, Clone)]
pub struct SingleEditOperation {
    /// The exact string to find and replace. Must be unique within the current state of the file content.
    /// If this is the first operation in a MultiEditTool call for a new file, or for EditTool if the file
    /// is being created, an empty string indicates that `new_string` should be used as the initial content.
    pub old_string: String,
    /// The string to replace `old_string` with.
    pub new_string: String,
}

/// Core logic for performing one or more edit operations on a file's content in memory.
/// This function handles file reading/creation setup, applies operations, and returns the new content.
/// It does NOT write to disk itself.
async fn perform_edit_operations(
    file_path_str: &str,
    operations: &[SingleEditOperation],
    token: Option<&CancellationToken>,
    tool_name_for_errors: &str,
) -> Result<(String, usize, bool), ToolError> {
    if let Some(t) = &token {
        if t.is_cancelled() {
            return Err(ToolError::Cancelled(tool_name_for_errors.to_string()));
        }
    }

    let path = Path::new(file_path_str);
    let mut current_content: String;
    let mut file_created_this_op = false;

    match fs::read_to_string(path).await {
        Ok(content_from_file) => {
            current_content = content_from_file;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if operations.is_empty() {
                return Err(ToolError::execution(
                    tool_name_for_errors,
                    format!(
                        "File {} does not exist and no operations provided to create it.",
                        file_path_str
                    ),
                ));
            }
            let first_op = &operations[0];
            if first_op.old_string.is_empty() {
                if let Some(parent) = path.parent() {
                    if !fs::metadata(parent)
                        .await
                        .map(|m| m.is_dir())
                        .unwrap_or(false)
                    {
                        if let Some(t) = &token {
                            if t.is_cancelled() {
                                return Err(ToolError::Cancelled(tool_name_for_errors.to_string()));
                            }
                        }
                        fs::create_dir_all(parent).await.map_err(|e| {
                            ToolError::io(
                                tool_name_for_errors,
                                format!("Failed to create directory {}: {}", parent.display(), e),
                            )
                        })?;
                    }
                }
                current_content = first_op.new_string.clone();
                file_created_this_op = true;
            } else {
                return Err(ToolError::io(
                    tool_name_for_errors,
                    format!(
                        "File {} not found, and the first/only operation's old_string is not empty (required for creation).",
                        file_path_str
                    ),
                ));
            }
        }
        Err(e) => {
            // Other read error
            return Err(ToolError::io(
                tool_name_for_errors,
                format!("Failed to read file {}: {}", file_path_str, e),
            ));
        }
    }

    if operations.is_empty() {
        // Should have been caught if file not found, but good for existing empty file
        return Ok((current_content, 0, false));
    }

    let mut edits_applied_count = 0;
    for (index, edit_op) in operations.iter().enumerate() {
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled(tool_name_for_errors.to_string()));
            }
        }

        if edit_op.old_string.is_empty() {
            if index == 0 && file_created_this_op {
                // This was the creation step; content is already set from edit_op.new_string.
            } else if index == 0 && operations.len() == 1 && edit_op.old_string.is_empty() {
                // This is a single "EditTool" style operation to overwrite/create the file
                current_content = edit_op.new_string.clone();
                if !file_created_this_op {
                    // If file existed and we are "creating" it with empty old_string
                    file_created_this_op = true; // Treat as creation for consistent messaging later
                }
            } else {
                return Err(ToolError::execution(
                    tool_name_for_errors,
                    format!(
                        "Edit #{} for file {} has an empty old_string. This is only allowed for the first operation if the file is being created or for a single operation to overwrite the file.",
                        index + 1,
                        file_path_str
                    ),
                ));
            }
        } else {
            // Normal replacement
            let occurrences = current_content.matches(&edit_op.old_string).count();
            if occurrences == 0 {
                return Err(ToolError::execution(
                    tool_name_for_errors,
                    format!(
                        "For edit #{}, string not found in file {} (after {} previous successful edits). String to find (first 50 chars): '{}'",
                        index + 1,
                        file_path_str,
                        edits_applied_count,
                        edit_op.old_string.chars().take(50).collect::<String>()
                    ),
                ));
            }
            if occurrences > 1 {
                return Err(ToolError::execution(
                    tool_name_for_errors,
                    format!(
                        "For edit #{}, found {} occurrences of string in file {} (after {} previous successful edits). String to find (first 50 chars): '{}'. Please provide more context.",
                        index + 1,
                        occurrences,
                        file_path_str,
                        edits_applied_count,
                        edit_op.old_string.chars().take(50).collect::<String>()
                    ),
                ));
            }
            current_content = current_content.replace(&edit_op.old_string, &edit_op.new_string);
        }
        edits_applied_count += 1;
    }
    Ok((current_content, edits_applied_count, file_created_this_op))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EditParams {
    /// The absolute path to the file to edit
    pub file_path: String,
    /// The exact string to find and replace. If empty, the file will be created.
    pub old_string: String,
    /// The string to replace `old_string` with.
    pub new_string: String,
}

tool! {
    EditTool {
        params: EditParams,
        description: r#"This is a tool for editing files. For moving or renaming files, you should generally use the Bash tool with the 'mv' command instead. For larger edits, use the replace tool to overwrite files.

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

Remember: when making multiple file edits in a row to the same file, you should prefer to send all edits in a single message with multiple calls to this tool, rather than multiple messages with a single call each."#,
        name: "edit_file",
        require_approval: true
    }

    async fn run(
        _tool: &EditTool,
        params: EditParams,
        context: &ExecutionContext,
    ) -> Result<String, ToolError> {
        let file_lock = get_file_lock(&params.file_path).await;
        let _lock_guard = file_lock.lock().await;

        let operation = SingleEditOperation {
            old_string: params.old_string,
            new_string: params.new_string,
        };

        match perform_edit_operations(&params.file_path, &[operation], Some(&context.cancellation_token), EDIT_TOOL_NAME).await {
            Ok((final_content, num_ops, created_or_overwritten)) => {
                // perform_edit_operations ensures num_ops is 1 if Ok for a single op, or 0 if it was a no-op on existing file.
                // It also handles the "creation" logic if old_string was empty.

                if created_or_overwritten || num_ops > 0 { // If created, or if existing file was modified
                    if context.cancellation_token.is_cancelled() { return Err(ToolError::Cancelled(EDIT_TOOL_NAME.to_string())); }
                    fs::write(Path::new(&params.file_path), &final_content)
                        .await
                        .map_err(|e| ToolError::io(EDIT_TOOL_NAME, format!("Failed to write file {}: {}", params.file_path, e)))?;

                    if created_or_overwritten {
                        // The "created_or_overwritten" flag from perform_edit_operations handles if old_string was empty.
                        Ok(format!("File created/overwritten: {}", params.file_path))
                    } else {
                        Ok(format!("File edited: {}", params.file_path))
                    }
                } else {
                     // This case implies the file existed, old_string was empty, new_string was same as existing content,
                     // or some other no-op scenario that perform_edit_operations handled by returning num_ops = 0 and created = false.
                    Ok(format!("File {} not changed (operation resulted in no change or was a no-op).", params.file_path))
                }
            }
            Err(e) => Err(e),
        }
    }
}

pub mod multi_edit {
    use super::*;

    #[derive(Deserialize, Debug, JsonSchema)]
    pub struct MultiEditParams {
        /// The absolute path to the file to edit.
        pub file_path: String,
        /// A list of edit operations to apply sequentially.
        pub edits: Vec<SingleEditOperation>,
    }

    tool! {
        MultiEditTool {
            params: MultiEditParams,
            description: format!(r#"This is a tool for making multiple edits to a single file in one operation. It is built on top of the {} tool and allows you to perform multiple find-and-replace operations efficiently. Prefer this tool over the {} tool when you need to make multiple edits to the same file.

Before using this tool:

1. Use the View tool to understand the file's contents and context
2. Verify the directory path is correct

To make multiple file edits, provide the following:
1. file_path: The absolute path to the file to modify (must be absolute, not relative)
2. edits: An array of edit operations to perform, where each edit contains:
   - old_string: The text to replace (must match the file contents exactly, including all whitespace and indentation)
   - new_string: The edited text to replace the old_string

IMPORTANT:
- All edits are applied in sequence, in the order they are provided
- Each edit operates on the result of the previous edit
- All edits must be valid for the operation to succeed - if any edit fails, none will be applied
- This tool is ideal when you need to make several changes to different parts of the same file

CRITICAL REQUIREMENTS:
1. All edits follow the same requirements as the single Edit tool
2. The edits are atomic - either all succeed or none are applied
3. Plan your edits carefully to avoid conflicts between sequential operations

WARNING: Since edits are applied in sequence, ensure that earlier edits don't affect the text that later edits are trying to find.

When making edits:
- Ensure all edits result in idiomatic, correct code
- Do not leave the code in a broken state
- Always use absolute file paths (starting with /)

If you want to create a new file, use:
- A new file path, including dir name if needed
- First edit: empty old_string and the new file's contents as new_string
- Subsequent edits: normal edit operations on the created content
"#, EDIT_TOOL_NAME, EDIT_TOOL_NAME),
            name: "multi_edit_file",
            require_approval: true
        }

        async fn run(
            _tool: &MultiEditTool,
            params: MultiEditParams,
            context: &ExecutionContext,
        ) -> Result<String, ToolError> {
            let file_lock = super::get_file_lock(&params.file_path).await;
            let _lock_guard = file_lock.lock().await;

            if params.edits.is_empty() {
                // If file exists, no change. If not, error.
                let path = Path::new(&params.file_path);
                 if !fs::metadata(path).await.map(|m| m.is_file()).unwrap_or(false) {
                     return Err(ToolError::execution(
                        MULTI_EDIT_TOOL_NAME,
                        format!("File {} does not exist and no edit operations provided to create or modify it.", params.file_path),
                    ));
                 }
                return Ok(format!("File {} not changed as no edits were provided.", params.file_path));
            }

            match super::perform_edit_operations(&params.file_path, &params.edits, Some(&context.cancellation_token), MULTI_EDIT_TOOL_NAME).await {
                Ok((final_content, num_ops_processed, file_was_created)) => {
                    // If perform_edit_operations returned Ok, it means all operations in params.edits were valid and processed.
                    // num_ops_processed should equal params.edits.len().
                    // The content is now ready to be written.

                    if num_ops_processed > 0 || file_was_created { // If any actual changes or creation happened
                        if context.cancellation_token.is_cancelled() { return Err(ToolError::Cancelled(MULTI_EDIT_TOOL_NAME.to_string())); }
                        fs::write(Path::new(&params.file_path), &final_content)
                            .await
                            .map_err(|e| ToolError::io(MULTI_EDIT_TOOL_NAME, format!("Failed to write file {}: {}", params.file_path, e)))?;

                        if file_was_created {
                            Ok(format!(
                                "File {} created and {} edit(s) applied successfully.",
                                params.file_path, num_ops_processed
                            ))
                        } else { // Edits were made to an existing file
                            Ok(format!(
                                "File {} edited successfully with {} operation(s).",
                                params.file_path, num_ops_processed
                            ))
                        }
                    } else {
                        // This case implies params.edits was not empty, but perform_edit_operations resulted in no effective change
                        // (e.g. all edits were no-ops that didn't change content and didn't create a file).
                        Ok(format!("File {} was not changed (operations resulted in no change).", params.file_path))
                    }
                }
                Err(e) => Err(e), // Propagate error from perform_edit_operations
            }
        }
    }
}
