use anyhow::{Context, Result};
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncReadExt; // For read_exact

// TODO: Refactor view_file to use async file I/O (tokio::fs)
// and check cancellation token periodically, especially when reading large files.
// Currently, fs::read_to_string can block on large files.

/// Read a file from the filesystem asynchronously
pub async fn view_file(
    file_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String> {
    let path = Path::new(file_path);

    // Verify file exists using tokio::fs
    if !fs::metadata(path)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        return Err(anyhow::anyhow!(
            "File not found or is not a file: {}",
            file_path
        ));
    }

    // Check if it's a binary file (also made async)
    if is_likely_binary(path).await? {
        return Ok(format!("Binary file: {}", file_path));
    }

    // Read file as string using tokio::fs
    let content = fs::read_to_string(path)
        .await
        .context(format!("Failed to read file: {}", file_path))?;

    // Apply offset and limit if specified
    let lines: Vec<&str> = content.lines().collect();
    let start = offset.unwrap_or(0);

    if start >= lines.len() {
        return Err(anyhow::anyhow!(
            "Offset {} exceeds file length ({} lines)",
            start,
            lines.len()
        ));
    }

    let end = limit
        .map(|l| std::cmp::min(start + l, lines.len()))
        .unwrap_or(lines.len());

    let result = lines[start..end].join("\n");

    // Append indication if we truncated the file
    let mut output = result;
    if end < lines.len() {
        output.push_str(&format!(
            "\n\n[Note: Showing lines {}-{} of {} total lines]",
            start,
            end.saturating_sub(1), // Use saturating_sub for safety
            lines.len()
        ));
    }

    Ok(output)
}

/// Check if a file is likely binary (async version)
async fn is_likely_binary(path: &Path) -> Result<bool> {
    // Check extension first (remains synchronous)
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        let binary_extensions = [
            "exe", "dll", "so", "dylib", "bin", "jpg", "jpeg", "png", "gif", "bmp", "ico", "pdf",
            "zip", "tar", "gz", "7z", "rar", "mp3", "mp4", "avi", "mov", "flv", "wav", "wma",
            "ogg", "class", "pyc", "o", "a", "lib", "obj", "pdb",
        ];

        if binary_extensions.contains(&ext_str.as_str()) {
            return Ok(true);
        }
    }

    // Get metadata asynchronously
    let metadata = fs::metadata(path).await?;
    if metadata.len() == 0 {
        return Ok(false); // Empty file
    }

    // Read first 8KB asynchronously
    let buffer_size = std::cmp::min(8192, metadata.len() as usize);
    let mut buffer = vec![0; buffer_size];
    let mut file = fs::File::open(path).await?;
    // Removed: use std::io::Read;
    file.read_exact(&mut buffer).await?;

    // Check for null bytes and other binary indicators (remains synchronous)
    Ok(buffer.iter().any(|&b| b == 0) || buffer.iter().filter(|&&b| b < 9).count() > 0)
}
