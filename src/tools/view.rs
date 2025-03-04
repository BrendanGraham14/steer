use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Read a file from the filesystem
pub fn view_file(file_path: &str, offset: Option<usize>, limit: Option<usize>) -> Result<String> {
    let path = Path::new(file_path);
    
    // Verify file exists
    if !path.exists() {
        return Err(anyhow::anyhow!("File not found: {}", file_path));
    }
    
    // Check if it's a binary file
    if is_likely_binary(path)? {
        return Ok(format!("Binary file: {}", file_path));
    }
    
    // Read file as string
    let content = fs::read_to_string(path)
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
            end - 1,
            lines.len()
        ));
    }
    
    Ok(output)
}

/// Check if a file is likely binary
fn is_likely_binary(path: &Path) -> Result<bool> {
    // Check extension first
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        let binary_extensions = [
            "exe", "dll", "so", "dylib", "bin", "jpg", "jpeg", "png", 
            "gif", "bmp", "ico", "pdf", "zip", "tar", "gz", "7z",
            "rar", "mp3", "mp4", "avi", "mov", "flv", "wav", "wma",
            "ogg", "class", "pyc", "o", "a", "lib", "obj", "pdb"
        ];
        
        if binary_extensions.contains(&ext_str.as_str()) {
            return Ok(true);
        }
    }
    
    // Read first 8KB to check for binary content
    let metadata = fs::metadata(path)?;
    if metadata.len() == 0 {
        return Ok(false); // Empty file
    }
    
    let mut buffer = vec![0; std::cmp::min(8192, metadata.len() as usize)];
    let mut file = fs::File::open(path)?;
    use std::io::Read;
    file.read_exact(&mut buffer)?;
    
    // Check for null bytes and other binary indicators
    Ok(buffer.iter().any(|&b| b == 0) || buffer.iter().filter(|&&b| b < 9).count() > 0)
}