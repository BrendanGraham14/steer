use serde::{Deserialize, Serialize};

/// Result for grep-like search tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub total_files_searched: usize,
    pub search_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub file_path: String,
    pub line_number: usize,
    pub line_content: String,
    pub column_range: Option<(usize, usize)>,
}

/// Result for file listing operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileListResult {
    pub entries: Vec<FileEntry>,
    pub base_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub permissions: Option<String>,
}

/// Result for file content viewing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContentResult {
    pub content: String,
    pub file_path: String,
    pub line_count: usize,
    pub truncated: bool,
}

/// Result for edit operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditResult {
    pub file_path: String,
    pub changes_made: usize,
    pub file_created: bool,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
}

/// Result for glob pattern matching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobResult {
    pub matches: Vec<String>,
    pub pattern: String,
}

/// Result for POSIX-style `wc` on a single file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WcResult {
    pub file_path: String,
    /// Number of newline (LF) bytes in the file.
    pub lines: u64,
    /// Number of whitespace-delimited words (ASCII whitespace; UTF-8 safe on raw bytes).
    pub words: u64,
    /// File size in bytes.
    pub bytes: u64,
}
