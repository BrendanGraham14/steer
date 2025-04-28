use anyhow::Result;
use chrono::Local;
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolError;
use coder_macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct LsParams {
    /// The absolute path to the directory to list
    path: String,
    /// Optional list of glob patterns to ignore
    ignore: Option<Vec<String>>,
}

tool! {
    LsTool {
        params: LsParams,
        description: "List files and directories in the workspace",
        name: "ls"
    }

    async fn run(
        _tool: &LsTool,
        params: LsParams,
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError> {
        // Cancellation check
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("LS".to_string()));
            }
        }

        // Call internal synchronous logic
        list_directory_internal(&params.path, &params.ignore.unwrap_or_default())
            .map_err(|e| ToolError::execution("LS", e))
    }
}

fn list_directory_internal(path_str: &str, ignore_patterns: &[String]) -> Result<String> {
    let path = Path::new(path_str);
    if !path.is_dir() {
        return Err(anyhow::anyhow!("Path is not a directory: {}", path_str));
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
    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for result in walker.skip(1) {
        // Skip the root directory itself
        match result {
            Ok(entry) => {
                let file_path = entry.path();
                let file_name = file_path.file_name().unwrap_or_default().to_string_lossy();

                // Add to appropriate list based on file type
                if file_path.is_dir() {
                    dirs.push(format!("{}/", file_name));
                } else {
                    files.push(file_name.to_string());
                }
            }
            Err(e) => {
                // Log errors but don't include in the output
                eprintln!("Error accessing entry: {}", e);
            }
        }
    }

    // Combine and sort all entries
    let mut all_entries = Vec::new();
    all_entries.extend(dirs);
    all_entries.extend(files);
    all_entries.sort();

    // Format output
    let mut output = String::new();

    if all_entries.is_empty() {
        output.push_str("Directory is empty or contains only ignored files.");
    } else {
        for entry in all_entries {
            output.push_str(&format!("{}\n", entry));
        }
        // Remove the last newline
        if output.ends_with('\n') {
            output.pop();
        }
    }

    Ok(output)
}
