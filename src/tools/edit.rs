use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolError;
use coder_macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct EditParams {
    /// The absolute path to the file to edit
    file_path: String,
    /// The exact string to find and replace. If empty, the file will be created.
    old_string: String,
    /// The string to replace `old_string` with.
    new_string: String,
}

tool! {
    EditTool {
        params: EditParams,
        description: "Edit a file by replacing an old string with a new string. Only works if the old string appears exactly once. If old_string is empty, creates the file."
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
