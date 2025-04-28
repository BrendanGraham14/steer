use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use crate::tools::ToolError;
use coder_macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct ViewParams {
    /// The absolute path to the file to read
    file_path: String,
    /// The line number to start reading from (1-indexed)
    offset: Option<u64>,
    /// The maximum number of lines to read
    limit: Option<u64>,
}

const MAX_READ_BYTES: usize = 50 * 1024; // Limit read size to 50KB

tool! {
    ViewTool {
        params: ViewParams,
        description: "Read a file from the local filesystem"
    }

    async fn run(
        _tool: &ViewTool,
        params: ViewParams,
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError> {
        // Cancellation check (can happen before calling view_file_internal)
        if let Some(t) = &token {
            if t.is_cancelled() {
                return Err(ToolError::Cancelled("View".to_string()));
            }
        }

        // Call internal async logic, passing the token
        view_file_internal(
            &params.file_path,
            params.offset.map(|v| v as usize),
            params.limit.map(|v| v as usize),
            token, // Pass token down for cancellation during read
        )
        .await
        // Map internal anyhow::Error to ToolError
        .map_err(|e| ToolError::io("View", e))
    }
}
async fn view_file_internal(
    file_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    token: Option<CancellationToken>,
) -> Result<String> {
    let mut file = File::open(file_path)
        .await
        .context(format!("Failed to open file: {}", file_path))?;

    let file_size = file.metadata().await?.len();
    let mut buffer = Vec::new();

    let start_line = offset.unwrap_or(1).max(1); // 1-indexed
    let line_limit = limit;

    if start_line > 1 || line_limit.is_some() {
        // Read line by line if offset or limit is specified
        let mut reader = tokio::io::BufReader::new(file);
        let mut current_line_num = 1;
        let mut lines_read = 0;
        let mut lines = Vec::new();

        loop {
            // Check for cancellation in the loop
            if let Some(t) = &token {
                if t.is_cancelled() {
                    return Err(anyhow::anyhow!("File read cancelled"));
                }
            }

            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if current_line_num >= start_line {
                        lines.push(line.trim_end().to_string()); // Store line
                        lines_read += 1;
                        if line_limit.map_or(false, |l| lines_read >= l) {
                            break; // Reached limit
                        }
                    }
                    current_line_num += 1;
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Error reading file line by line: {}", e));
                }
            }
        }
        buffer = lines.join("\n").into_bytes();
    } else {
        // Read the whole file (up to MAX_READ_BYTES)
        let read_size = std::cmp::min(file_size as usize, MAX_READ_BYTES);
        buffer.resize(read_size, 0);
        let mut bytes_read = 0;
        while bytes_read < read_size {
            // Check for cancellation in the loop
            if let Some(t) = &token {
                if t.is_cancelled() {
                    return Err(anyhow::anyhow!("File read cancelled"));
                }
            }
            let n = file.read(&mut buffer[bytes_read..]).await?;
            if n == 0 {
                break; // EOF
            }
            bytes_read += n;
        }
        buffer.truncate(bytes_read);
    }

    // Attempt to decode as UTF-8, lossy conversion as fallback
    Ok(String::from_utf8_lossy(&buffer).to_string())
}
