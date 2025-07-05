use conductor_macros::tool;
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
use tokio::task;

use crate::result::{FileEntry, FileListResult};
use crate::{ExecutionContext, ToolError};

#[derive(Debug, Error)]
pub enum LsError {
    #[error("Path is not a directory: {path}")]
    NotADirectory { path: String },

    #[error("Operation was cancelled")]
    Cancelled,

    #[error("Task join error: {source}")]
    TaskJoinError {
        #[from]
        #[source]
        source: tokio::task::JoinError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LsParams {
    /// The absolute path to the directory to list (must be absolute, not relative)
    pub path: String,
    /// Optional list of glob patterns to ignore
    pub ignore: Option<Vec<String>>,
}

tool! {
    LsTool {
        params: LsParams,
        output: FileListResult,
        variant: FileList,
        description: "Lists files and directories in a given path. The path parameter must be an absolute path, not a relative path. You should generally prefer the Glob and Grep tools, if you know which directories to search.",
        name: "ls",
        require_approval: false
    }

    async fn run(
        _tool: &LsTool,
        params: LsParams,
        context: &ExecutionContext,
    ) -> Result<FileListResult, ToolError> {
        if context.is_cancelled() {
            return Err(ToolError::Cancelled(LS_TOOL_NAME.to_string()));
        }

        let target_path = if Path::new(&params.path).is_absolute() {
            params.path.clone()
        } else {
            context.working_directory.join(&params.path)
                .to_string_lossy()
                .to_string()
        };

        let ignore_patterns = params.ignore.unwrap_or_default();
        let cancellation_token = context.cancellation_token.clone();

        // Run the blocking directory listing in a separate task
        let result = task::spawn_blocking(move || {
            list_directory_internal(&target_path, &ignore_patterns, &cancellation_token)
        }).await;

        match result {
            Ok(listing_result) => listing_result.map_err(|e| ToolError::execution(LS_TOOL_NAME, e.to_string())),
            Err(join_error) => {
                let ls_error = LsError::TaskJoinError { source: join_error };
                Err(ToolError::execution(LS_TOOL_NAME, ls_error.to_string()))
            }
        }
    }
}

fn list_directory_internal(
    path_str: &str,
    ignore_patterns: &[String],
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<FileListResult, LsError> {
    let path = Path::new(path_str);
    if !path.is_dir() {
        return Err(LsError::NotADirectory {
            path: path_str.to_string(),
        });
    }

    if cancellation_token.is_cancelled() {
        return Err(LsError::Cancelled);
    }

    let mut walk_builder = WalkBuilder::new(path);
    walk_builder.max_depth(Some(1)); // Only list immediate children
    walk_builder.git_ignore(true);
    walk_builder.ignore(true);
    walk_builder.hidden(false); // Show hidden files unless explicitly ignored

    // Add custom ignore patterns
    for pattern in ignore_patterns {
        walk_builder.add_ignore(pattern);
    }

    let walker = walk_builder.build();
    let mut entries = Vec::new();

    for result in walker.skip(1) {
        // Skip the root directory itself
        if cancellation_token.is_cancelled() {
            return Err(LsError::Cancelled);
        }

        match result {
            Ok(entry) => {
                let file_path = entry.path();
                let file_name = file_path.file_name().unwrap_or_default().to_string_lossy();

                // Create FileEntry
                let metadata = file_path.metadata().ok();
                let size = if file_path.is_dir() {
                    None
                } else {
                    metadata.as_ref().map(|m| m.len())
                };

                entries.push(FileEntry {
                    path: file_name.to_string(),
                    is_directory: file_path.is_dir(),
                    size,
                    permissions: None, // Could add if needed
                });
            }
            Err(e) => {
                // Log errors but don't include in the output
                eprintln!("Error accessing entry: {e}");
            }
        }
    }

    // Sort entries by name
    entries.sort_by(|a, b| {
        // Directories first, then files
        match (a.is_directory, b.is_directory) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.path.cmp(&b.path),
        }
    });

    Ok(FileListResult {
        entries,
        base_path: path_str.to_string(),
    })
}
