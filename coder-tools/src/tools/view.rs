use coder_macros::tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::{ExecutionContext, ToolError};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ViewParams {
    /// The absolute path to the file to read
    pub file_path: String,
    /// The line number to start reading from (1-indexed)
    pub offset: Option<u64>,
    /// The maximum number of lines to read
    pub limit: Option<u64>,
}

#[derive(Error, Debug)]
enum ViewError {
    #[error("Failed to open file '{path}': {source}")]
    FileOpen {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Failed to get file metadata for '{path}': {source}")]
    Metadata {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("File read cancelled")]
    Cancelled,
    #[error("Error reading file line by line: {source}")]
    ReadLine {
        #[source]
        source: std::io::Error,
    },
    #[error("Error reading file: {source}")]
    Read {
        #[source]
        source: std::io::Error,
    },
}

const MAX_READ_BYTES: usize = 50 * 1024; // Limit read size to 50KB
const MAX_LINE_LENGTH: usize = 2000; // Maximum characters per line before truncation

tool! {
    ViewTool {
        params: ViewParams,
        description: format!(r#"Reads a file from the local filesystem. The file_path parameter must be an absolute path, not a relative path.
By default, it reads up to 2000 lines starting from the beginning of the file. You can optionally specify a line offset and limit
(especially handy for long files), but it's recommended to read the whole file by not providing these parameters.
Any lines longer than {} characters will be truncated."#, MAX_LINE_LENGTH),
        name: "read_file",
        require_approval: false
    }

    async fn run(
        _tool: &ViewTool,
        params: ViewParams,
        context: &ExecutionContext,
    ) -> Result<String, ToolError> {
        if context.is_cancelled() {
            return Err(ToolError::Cancelled(VIEW_TOOL_NAME.to_string()));
        }

        // Convert to absolute path relative to working directory
        let abs_path = if Path::new(&params.file_path).is_absolute() {
            params.file_path.clone()
        } else {
            context.working_directory.join(&params.file_path)
                .to_string_lossy()
                .to_string()
        };

        view_file_internal(
            &abs_path,
            params.offset.map(|v| v as usize),
            params.limit.map(|v| v as usize),
            context,
        )
        .await
        .map_err(|e| ToolError::io(VIEW_TOOL_NAME, e.to_string()))
    }
}

async fn view_file_internal(
    file_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    context: &ExecutionContext,
) -> Result<String, ViewError> {
    let mut file = File::open(file_path)
        .await
        .map_err(|e| ViewError::FileOpen {
            path: file_path.to_string(),
            source: e,
        })?;

    let file_size = file
        .metadata()
        .await
        .map_err(|e| ViewError::Metadata {
            path: file_path.to_string(),
            source: e,
        })?
        .len();

    let mut buffer = Vec::new();
    let start_line = offset.unwrap_or(1).max(1); // 1-indexed
    let line_limit = limit;

    if start_line > 1 || line_limit.is_some() {
        // Read line by line if offset or limit is specified
        let mut reader = BufReader::new(file);
        let mut current_line_num = 1;
        let mut lines_read = 0;
        let mut lines = Vec::new();

        loop {
            // Check for cancellation in the loop
            if context.is_cancelled() {
                return Err(ViewError::Cancelled);
            }

            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if current_line_num >= start_line {
                        // Truncate long lines
                        if line.len() > MAX_LINE_LENGTH {
                            line.truncate(MAX_LINE_LENGTH);
                            line.push_str("... [line truncated]");
                        }
                        lines.push(line.trim_end().to_string()); // Store line
                        lines_read += 1;
                        if line_limit.is_some_and(|l| lines_read >= l) {
                            break; // Reached limit
                        }
                    }
                    current_line_num += 1;
                }
                Err(e) => {
                    return Err(ViewError::ReadLine { source: e });
                }
            }
        }

        // Add line numbers
        let numbered_lines: Vec<String> = lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| format!("{:5}\t{}", start_line + i, line))
            .collect();

        buffer = numbered_lines.join("\n").into_bytes();
    } else {
        // Read the whole file (up to MAX_READ_BYTES)
        let read_size = std::cmp::min(file_size as usize, MAX_READ_BYTES);
        buffer.resize(read_size, 0);
        let mut bytes_read = 0;
        while bytes_read < read_size {
            // Check for cancellation in the loop
            if context.is_cancelled() {
                return Err(ViewError::Cancelled);
            }
            let n = file
                .read(&mut buffer[bytes_read..])
                .await
                .map_err(|e| ViewError::Read { source: e })?;
            if n == 0 {
                break; // EOF
            }
            bytes_read += n;
        }
        buffer.truncate(bytes_read);

        // Convert to string and add line numbers
        let content = String::from_utf8_lossy(&buffer);
        let lines: Vec<&str> = content.lines().collect();
        let numbered_lines: Vec<String> = lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| {
                // Truncate long lines
                let truncated_line = if line.len() > MAX_LINE_LENGTH {
                    format!("{}... [line truncated]", &line[..MAX_LINE_LENGTH])
                } else {
                    line.to_string()
                };
                format!("{:5}\t{}", i + 1, truncated_line)
            })
            .collect();

        buffer = numbered_lines.join("\n").into_bytes();
    }

    // Return the final content
    Ok(String::from_utf8_lossy(&buffer).to_string())
}
