use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolError;
use coder_macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
pub struct ReplaceParams {
    /// The absolute path to the file to write
    pub file_path: String,
    /// The content to write to the file
    pub content: String,
}

tool! {
    ReplaceTool {
        params: ReplaceParams,
        description: "Write a file to the local filesystem, replacing it if it exists.",
        name: "replace_file"
    }

    async fn run(
        _tool: &ReplaceTool,
        params: ReplaceParams,
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError> {
        // Cancellation check before starting
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("Replace".to_string()));
            }
        }

        let path = Path::new(&params.file_path);

        // Ensure parent directory exists asynchronously
        if let Some(parent) = path.parent() {
            // Check cancellation before directory metadata check
            if let Some(t) = &token {
                if t.is_cancelled() {
                    return Err(ToolError::Cancelled("Replace".to_string()));
                }
            }
            if !fs::metadata(parent)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                 // Check cancellation before creating directory
                if let Some(t) = &token {
                    if t.is_cancelled() {
                        return Err(ToolError::Cancelled("Replace".to_string()));
                    }
                }
                fs::create_dir_all(parent)
                    .await
                    .context(format!("Failed to create directory: {}", parent.display()))
                    .map_err(|e| ToolError::io("Replace", e))?;
            }
        }

        // Check cancellation before writing file
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("Replace".to_string()));
            }
        }

        // Write the file asynchronously
        fs::write(path, &params.content)
            .await
            .context(format!("Failed to write file: {}", params.file_path))
            .map_err(|e| ToolError::io("Replace", e))?;

        Ok(format!("File written: {}", params.file_path))
    }
}
