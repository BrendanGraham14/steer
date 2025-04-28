use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolError;
use coder_macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
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
        description: r#"This is a tool for editing files. For moving or renaming files, you should generally use the Bash tool with the 'mv' command instead. For larger edits, use the replace tool to overwrite files. For Jupyter notebooks (.ipynb files), use the NotebookEditCell instead.

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
        name: "edit_file"
    }

    async fn run(
        _tool: &EditTool,
        params: EditParams,
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError> {
        // Initial cancellation check
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("Edit".to_string()));
            }
        }

        let path = Path::new(&params.file_path);

        // Handle new file creation
        if params.old_string.is_empty() {
            // Ensure parent directory exists asynchronously
            if let Some(parent) = path.parent() {
                 // Cancellation check
                if let Some(t) = &token {
                    if t.is_cancelled() {
                        return Err(ToolError::Cancelled("Edit".to_string()));
                    }
                }
                if !fs::metadata(parent)
                    .await
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
                {
                    // Cancellation check
                    if let Some(t) = &token {
                        if t.is_cancelled() {
                            return Err(ToolError::Cancelled("Edit".to_string()));
                        }
                    }
                    fs::create_dir_all(parent)
                        .await
                        .context(format!("Failed to create directory: {}", parent.display()))
                        .map_err(|e| ToolError::io("Edit", e))?;
                }
            }

             // Cancellation check
            if let Some(t) = &token {
                if t.is_cancelled() {
                    return Err(ToolError::Cancelled("Edit".to_string()));
                }
            }
            // Write the new file asynchronously
            fs::write(path, &params.new_string)
                .await
                .context(format!("Failed to create file: {}", params.file_path))
                .map_err(|e| ToolError::io("Edit", e))?;

            return Ok(format!("File created: {}", params.file_path));
        }

        // Check if file exists for editing asynchronously
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("Edit".to_string()));
            }
        }
        if !fs::metadata(path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            return Err(ToolError::io(
                "Edit",
                anyhow::anyhow!("File not found or is not a file: {}", params.file_path),
            ));
        }

        // Read the file content asynchronously
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("Edit".to_string()));
            }
        }
        let content = fs::read_to_string(path)
            .await
            .context(format!("Failed to read file: {}", params.file_path))
            .map_err(|e| ToolError::io("Edit", e))?;

        // Count occurrences of the old string (synchronous, but should be fast)
        let occurrences = content.matches(&params.old_string).count();

        if occurrences == 0 {
            return Err(ToolError::execution(
                "Edit",
                anyhow::anyhow!("String not found in file: {}", params.file_path),
            ));
        }

        if occurrences > 1 {
            return Err(ToolError::execution(
                "Edit",
                anyhow::anyhow!(
                    "Found {} occurrences of the string in file: {}. Please provide more context to uniquely identify the instance to replace.",
                    occurrences,
                    params.file_path
                ),
            ));
        }

        // Replace the string (synchronous, but should be fast)
        let new_content = content.replace(&params.old_string, &params.new_string);

        // Write the updated content asynchronously
         if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("Edit".to_string()));
            }
        }
        fs::write(path, new_content)
            .await
            .context(format!("Failed to write file: {}", params.file_path))
            .map_err(|e| ToolError::io("Edit", e))?;

        Ok(format!("File edited: {}", params.file_path))
    }
}

// TODO: Refactor edit_file to use async file I/O (tokio::fs)
// and potentially check cancellation token, especially for large files.
// Currently, fs::read_to_string and fs::write can block. <-- This comment is now outdated.

// /// Edit a file by replacing an old string with a new string (async)
// pub async fn edit_file(file_path: &str, old_string: &str, new_string: &str) -> Result<String> {
//     let path = Path::new(file_path);
//
//     // Handle new file creation
//     if old_string.is_empty() {
//         // Ensure parent directory exists asynchronously
//         if let Some(parent) = path.parent() {
//             if !fs::metadata(parent)
//                 .await
//                 .map(|m| m.is_dir())
//                 .unwrap_or(false)
//             {
//                 fs::create_dir_all(parent)
//                     .await
//                     .context(format!("Failed to create directory: {}", parent.display()))?;
//             }
//         }
//
//         // Write the new file asynchronously
//         fs::write(path, new_string)
//             .await
//             .context(format!("Failed to create file: {}", file_path))?;
//
//         return Ok(format!("File created: {}", file_path));
//     }
//
//     // Check if file exists for editing asynchronously
//     if !fs::metadata(path)
//         .await
//         .map(|m| m.is_file())
//         .unwrap_or(false)
//     {
//         return Err(anyhow::anyhow!(
//             "File not found or is not a file: {}",
//             file_path
//         ));
//     }
//
//     // Read the file content asynchronously
//     let content = fs::read_to_string(path)
//         .await
//         .context(format!("Failed to read file: {}", file_path))?;
//
//     // Count occurrences of the old string (synchronous)
//     let occurrences = content.matches(old_string).count();
//
//     if occurrences == 0 {
//         return Err(anyhow::anyhow!("String not found in file: {}", file_path));
//     }
//
//     if occurrences > 1 {
//         return Err(anyhow::anyhow!(
//             "Found {} occurrences of the string in file: {}. Please provide more context to uniquely identify the instance to replace.",
//             occurrences,
//             file_path
//         ));
//     }
//
//     // Replace the string (synchronous)
//     let new_content = content.replace(old_string, new_string);
//
//     // Write the updated content asynchronously
//     fs::write(path, new_content)
//         .await
//         .context(format!("Failed to write file: {}", file_path))?;
//
//     Ok(format!("File edited: {}", file_path))
// }
