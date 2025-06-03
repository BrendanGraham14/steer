use coder_macros::tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;

use crate::tools::{LS_TOOL_NAME, VIEW_TOOL_NAME};
use crate::{ExecutionContext, ToolError};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReplaceParams {
    /// The absolute path to the file to write (must be absolute, not relative)
    pub file_path: String,
    /// The content to write to the file
    pub content: String,
}

tool! {
    ReplaceTool {
        params: ReplaceParams,
        description: format!(r#"Writes a file to the local filesystem.

Before using this tool:

1. Use the {} tool to understand the file's contents and context

2. Directory Verification (only applicable when creating new files):
 - Use the {} tool to verify the parent directory exists and is the correct location"#, VIEW_TOOL_NAME, LS_TOOL_NAME),
        name: "write_file",
        require_approval: true
    }

    async fn run(
        _tool: &ReplaceTool,
        params: ReplaceParams,
        context: &ExecutionContext,
    ) -> Result<String, ToolError> {
        // Validate absolute path
        if !params.file_path.starts_with('/') && !params.file_path.starts_with("\\") {
            return Err(ToolError::invalid_params(
                REPLACE_TOOL_NAME,
                "file_path must be an absolute path".to_string(),
            ));
        }

        // Convert to absolute path relative to working directory
        let abs_path = if Path::new(&params.file_path).is_absolute() {
            params.file_path.clone()
        } else {
            context
                .working_directory
                .join(&params.file_path)
                .to_string_lossy()
                .to_string()
        };

        let path = Path::new(&abs_path);

        if context.is_cancelled() {
            return Err(ToolError::Cancelled(REPLACE_TOOL_NAME.to_string()));
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    ToolError::io(
                        REPLACE_TOOL_NAME,
                        format!("Failed to create parent directory: {}", e),
                    )
                })?;
            }
        }

        // Write the file
        fs::write(path, &params.content).await.map_err(|e| {
            ToolError::io(
                REPLACE_TOOL_NAME,
                format!("Failed to write file {}: {}", abs_path, e),
            )
        })?;

        Ok(format!("File written successfully: {}", abs_path))
    }
}
