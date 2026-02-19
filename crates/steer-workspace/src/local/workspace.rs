use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::{Mutex, RwLock};
use tokio::task;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::error::{Result as WorkspaceResult, WorkspaceError};
use crate::ops::{
    ApplyEditsRequest, AstGrepRequest, GlobRequest, GrepRequest, ListDirectoryRequest,
    ReadFileRequest, WorkspaceOpContext, WriteFileRequest,
};
use crate::result::{
    EditResult, FileContentResult, FileEntry, FileListResult, GlobResult, SearchMatch, SearchResult,
};
use crate::{CachedEnvironment, EnvironmentInfo, Workspace, WorkspaceMetadata, WorkspaceType};

use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_core::{AstGrep, Pattern};
use ast_grep_language::{LanguageExt, SupportLang};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder, SinkError};
use ignore::WalkBuilder;

/// Local filesystem workspace
pub struct LocalWorkspace {
    path: PathBuf,
    environment_cache: Arc<RwLock<Option<CachedEnvironment>>>,
    metadata: WorkspaceMetadata,
}

const MAX_READ_BYTES: usize = 50 * 1024;
const MAX_LINE_LENGTH: usize = 2000;

static FILE_LOCKS: std::sync::LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

async fn get_file_lock(file_path: &str) -> Arc<Mutex<()>> {
    let mut locks_map_guard = FILE_LOCKS.lock().await;
    locks_map_guard
        .entry(file_path.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn resolve_path(base: &Path, path: &str) -> PathBuf {
    if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        base.join(path)
    }
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

#[derive(Error, Debug)]
enum LsError {
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

async fn view_file_internal(
    file_path: &Path,
    offset: Option<u64>,
    limit: Option<u64>,
    cancellation_token: &CancellationToken,
) -> std::result::Result<FileContentResult, ViewError> {
    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|e| ViewError::FileOpen {
            path: file_path.display().to_string(),
            source: e,
        })?;

    let file_size = file
        .metadata()
        .await
        .map_err(|e| ViewError::Metadata {
            path: file_path.display().to_string(),
            source: e,
        })?
        .len();

    let start_line = offset.unwrap_or(1).max(1) as usize;
    let line_limit = limit.map(|v| v.max(1) as usize);

    let (content, total_lines, truncated) = if start_line > 1 || line_limit.is_some() {
        let mut reader = BufReader::new(file);
        let mut current_line_num = 1usize;
        let mut lines_read = 0usize;
        let mut lines = Vec::new();

        loop {
            if cancellation_token.is_cancelled() {
                return Err(ViewError::Cancelled);
            }

            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    if current_line_num >= start_line {
                        if line.len() > MAX_LINE_LENGTH {
                            line.truncate(MAX_LINE_LENGTH);
                            line.push_str("... [line truncated]");
                        }
                        lines.push(line.trim_end().to_string());
                        lines_read += 1;
                        if line_limit.is_some_and(|l| lines_read >= l) {
                            break;
                        }
                    }
                    current_line_num += 1;
                }
                Err(e) => return Err(ViewError::ReadLine { source: e }),
            }
        }

        let total_lines = lines.len();
        let truncated = line_limit.is_some_and(|l| lines_read >= l);
        let numbered_lines: Vec<String> = lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| format!("{:5}\t{}", start_line + i, line))
            .collect();

        (numbered_lines.join("\n"), total_lines, truncated)
    } else {
        let read_size = std::cmp::min(file_size as usize, MAX_READ_BYTES);
        let mut buffer = vec![0u8; read_size];
        let mut bytes_read = 0usize;

        while bytes_read < read_size {
            if cancellation_token.is_cancelled() {
                return Err(ViewError::Cancelled);
            }
            let n = file
                .read(&mut buffer[bytes_read..])
                .await
                .map_err(|e| ViewError::Read { source: e })?;
            if n == 0 {
                break;
            }
            bytes_read += n;
        }

        buffer.truncate(bytes_read);
        let content = String::from_utf8_lossy(&buffer);
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let truncated = file_size as usize > MAX_READ_BYTES;
        let numbered_lines: Vec<String> = lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| format!("{:5}\t{}", i + 1, line))
            .collect();

        (numbered_lines.join("\n"), total_lines, truncated)
    };

    Ok(FileContentResult {
        content,
        file_path: file_path.display().to_string(),
        line_count: total_lines,
        truncated,
    })
}

fn list_directory_internal(
    path_str: &str,
    ignore_patterns: &[String],
    cancellation_token: &CancellationToken,
) -> std::result::Result<FileListResult, LsError> {
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
    walk_builder.max_depth(Some(1));
    walk_builder.git_ignore(true);
    walk_builder.ignore(true);
    walk_builder.hidden(false);

    for pattern in ignore_patterns {
        walk_builder.add_ignore(pattern);
    }

    let walker = walk_builder.build();
    let mut entries = Vec::new();

    for result in walker.skip(1) {
        if cancellation_token.is_cancelled() {
            return Err(LsError::Cancelled);
        }

        match result {
            Ok(entry) => {
                let file_path = entry.path();
                let file_name = file_path.file_name().unwrap_or_default().to_string_lossy();
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
                    permissions: None,
                });
            }
            Err(e) => {
                tracing::warn!("Error accessing entry: {e}");
            }
        }
    }

    entries.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.path.cmp(&b.path),
    });

    Ok(FileListResult {
        entries,
        base_path: path_str.to_string(),
    })
}

fn grep_search_internal(
    pattern: &str,
    include: Option<&str>,
    base_path: &Path,
    cancellation_token: &CancellationToken,
) -> std::result::Result<SearchResult, String> {
    struct FileMatchBucket {
        mtime: std::time::SystemTime,
        matches: Vec<(usize, String)>,
    }

    if !base_path.exists() {
        return Err(format!("Path does not exist: {}", base_path.display()));
    }

    let matcher_pattern = if RegexMatcherBuilder::new()
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .is_ok()
    {
        pattern.to_string()
    } else {
        let escaped = regex::escape(pattern);
        RegexMatcherBuilder::new()
            .line_terminator(Some(b'\n'))
            .build(&escaped)
            .map_err(|e| format!("Failed to create matcher: {e}"))?;
        escaped
    };

    let include_glob = include.map(ToOwned::to_owned);
    if let Some(include_pattern) = include_glob.as_deref() {
        glob::Pattern::new(include_pattern).map_err(|e| format!("Invalid glob pattern: {e}"))?;
    }

    let mut walker = WalkBuilder::new(base_path);
    walker.hidden(false);
    walker.git_ignore(true);
    walker.git_global(true);
    walker.git_exclude(true);

    let include_pattern = include
        .map(|p| glob::Pattern::new(p).map_err(|e| format!("Invalid glob pattern: {e}")))
        .transpose()?;

    let mut file_buckets: BTreeMap<String, FileMatchBucket> = BTreeMap::new();
    let mut files_searched = 0usize;

    for result in walker.build() {
        if cancellation_token.is_cancelled() {
            break;
        }

        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        if let Some(ref pattern) = include_pattern
            && !path_matches_glob(path, pattern, base_path)
        {
            continue;
        }

        files_searched += 1;

        let display_path = match path.canonicalize() {
            Ok(canonical) => canonical.display().to_string(),
            Err(_) => path.display().to_string(),
        };
        let file_mtime = path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        let mut lines_in_file = Vec::new();
        let matcher = RegexMatcherBuilder::new()
            .line_terminator(Some(b'\n'))
            .build(&matcher_pattern)
            .map_err(|e| format!("Failed to create matcher: {e}"))?;
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .line_number(true)
            .build();

        let search_result = searcher.search_path(
            &matcher,
            path,
            UTF8(|line_num, line| {
                if cancellation_token.is_cancelled() {
                    return Err(SinkError::error_message("Operation cancelled".to_string()));
                }

                lines_in_file.push((line_num as usize, line.trim_end().to_string()));
                Ok(true)
            }),
        );

        let append_file_matches =
            |buckets: &mut BTreeMap<String, FileMatchBucket>,
             file_matches: Vec<(usize, String)>| {
                if file_matches.is_empty() {
                    return;
                }

                let bucket =
                    buckets
                        .entry(display_path.clone())
                        .or_insert_with(|| FileMatchBucket {
                            mtime: file_mtime,
                            matches: Vec::new(),
                        });
                if file_mtime > bucket.mtime {
                    bucket.mtime = file_mtime;
                }
                bucket.matches.extend(file_matches);
            };

        match search_result {
            Err(err)
                if cancellation_token.is_cancelled()
                    && err.to_string().contains("Operation cancelled") =>
            {
                append_file_matches(&mut file_buckets, lines_in_file);
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {}
            Err(_) | Ok(()) => {
                append_file_matches(&mut file_buckets, lines_in_file);
            }
        }
    }

    let search_completed = !cancellation_token.is_cancelled();
    if file_buckets.is_empty() {
        return Ok(SearchResult {
            matches: Vec::new(),
            total_files_searched: files_searched,
            search_completed,
        });
    }

    let mut sorted_files: Vec<(String, FileMatchBucket)> = file_buckets.into_iter().collect();
    if sorted_files.len() > 1 {
        sorted_files.sort_by(|a, b| b.1.mtime.cmp(&a.1.mtime).then_with(|| a.0.cmp(&b.0)));
    }

    let total_matches = sorted_files
        .iter()
        .map(|(_, bucket)| bucket.matches.len())
        .sum();
    let mut matches = Vec::with_capacity(total_matches);
    for (file_path, mut bucket) in sorted_files {
        for (line_number, line_content) in bucket.matches.drain(..) {
            matches.push(SearchMatch {
                file_path: file_path.clone(),
                line_number,
                line_content,
                column_range: None,
            });
        }
    }

    Ok(SearchResult {
        matches,
        total_files_searched: files_searched,
        search_completed,
    })
}

fn astgrep_search_internal(
    pattern: &str,
    lang: Option<&str>,
    include: Option<&str>,
    exclude: Option<&str>,
    base_path: &Path,
    cancellation_token: &CancellationToken,
) -> std::result::Result<SearchResult, String> {
    if !base_path.exists() {
        return Err(format!("Path does not exist: {}", base_path.display()));
    }

    let mut walker = WalkBuilder::new(base_path);
    walker.hidden(false);
    walker.git_ignore(true);
    walker.git_global(true);
    walker.git_exclude(true);

    let include_pattern = include
        .map(|p| glob::Pattern::new(p).map_err(|e| format!("Invalid include glob pattern: {e}")))
        .transpose()?;

    let exclude_pattern = exclude
        .map(|p| glob::Pattern::new(p).map_err(|e| format!("Invalid exclude glob pattern: {e}")))
        .transpose()?;

    let mut all_matches = Vec::new();
    let mut files_searched = 0usize;

    for result in walker.build() {
        if cancellation_token.is_cancelled() {
            return Ok(SearchResult {
                matches: all_matches,
                total_files_searched: files_searched,
                search_completed: false,
            });
        }

        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        if let Some(ref pattern) = include_pattern
            && !path_matches_glob(path, pattern, base_path)
        {
            continue;
        }

        if let Some(ref pattern) = exclude_pattern
            && path_matches_glob(path, pattern, base_path)
        {
            continue;
        }

        let detected_lang = if let Some(l) = lang {
            match SupportLang::from_str(l) {
                Ok(lang) => Some(lang),
                Err(_) => continue,
            }
        } else {
            SupportLang::from_extension(path).or_else(|| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(|ext| match ext {
                        "jsx" => Some(SupportLang::JavaScript),
                        "mjs" => Some(SupportLang::JavaScript),
                        _ => None,
                    })
            })
        };

        let Some(language) = detected_lang else {
            continue;
        };

        files_searched += 1;
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let ast_grep = language.ast_grep(&content);
        let pattern_matcher = match Pattern::try_new(pattern, language) {
            Ok(p) => p,
            Err(e) => return Err(format!("Invalid pattern: {e}")),
        };

        let relative_path = path.strip_prefix(base_path).unwrap_or(path);
        let file_matches = find_matches(&ast_grep, &pattern_matcher, relative_path, &content);

        for m in file_matches {
            all_matches.push(SearchMatch {
                file_path: m.file,
                line_number: m.line,
                line_content: m.context.trim().to_string(),
                column_range: Some((m.column, m.column + m.matched_code.len())),
            });
        }
    }

    all_matches.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.line_number.cmp(&b.line_number))
    });

    Ok(SearchResult {
        matches: all_matches,
        total_files_searched: files_searched,
        search_completed: true,
    })
}

#[derive(Debug)]
struct AstGrepMatch {
    file: String,
    line: usize,
    column: usize,
    matched_code: String,
    context: String,
}

fn find_matches(
    ast_grep: &AstGrep<StrDoc<SupportLang>>,
    pattern: &Pattern,
    path: &Path,
    content: &str,
) -> Vec<AstGrepMatch> {
    let root = ast_grep.root();
    let matches = root.find_all(pattern);

    let mut results = Vec::new();
    for node_match in matches {
        let node = node_match.get_node();
        let range = node.range();
        let start_pos = node.start_pos();
        let matched_code = node.text();

        let line_start = content[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = content[range.end..]
            .find('\n')
            .map_or(content.len(), |i| range.end + i);
        let context = &content[line_start..line_end];

        results.push(AstGrepMatch {
            file: path.display().to_string(),
            line: start_pos.line() + 1,
            column: start_pos.column(node) + 1,
            matched_code: matched_code.to_string(),
            context: context.to_string(),
        });
    }

    results
}

fn path_matches_glob(path: &Path, pattern: &glob::Pattern, base_path: &Path) -> bool {
    if pattern.matches_path(path) {
        return true;
    }

    if let Ok(relative_path) = path.strip_prefix(base_path)
        && pattern.matches_path(relative_path)
    {
        return true;
    }

    if let Some(filename) = path.file_name()
        && pattern.matches(&filename.to_string_lossy())
    {
        return true;
    }

    false
}

trait LanguageHelpers {
    fn from_extension(path: &Path) -> Option<SupportLang>;
}

impl LanguageHelpers for SupportLang {
    fn from_extension(path: &Path) -> Option<SupportLang> {
        ast_grep_language::Language::from_path(path)
    }
}

async fn perform_edit_operations(
    file_path: &Path,
    operations: &[crate::ops::EditOperation],
    token: Option<&CancellationToken>,
) -> WorkspaceResult<(String, usize, bool)> {
    if token.is_some_and(|t| t.is_cancelled()) {
        return Err(WorkspaceError::ToolExecution(
            "Operation cancelled".to_string(),
        ));
    }

    let mut current_content: String;
    let mut file_created_this_op = false;

    match tokio::fs::read_to_string(file_path).await {
        Ok(content_from_file) => {
            current_content = content_from_file;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if operations.is_empty() {
                return Err(WorkspaceError::ToolExecution(format!(
                    "File {} does not exist and no operations provided to create it.",
                    file_path.display()
                )));
            }
            let first_op = &operations[0];
            if first_op.old_string.is_empty() {
                if let Some(parent) = file_path.parent()
                    && !tokio::fs::metadata(parent)
                        .await
                        .map(|m| m.is_dir())
                        .unwrap_or(false)
                {
                    if token.is_some_and(|t| t.is_cancelled()) {
                        return Err(WorkspaceError::ToolExecution(
                            "Operation cancelled".to_string(),
                        ));
                    }
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        WorkspaceError::Io(format!(
                            "Failed to create directory {}: {}",
                            parent.display(),
                            e
                        ))
                    })?;
                }
                current_content = first_op.new_string.clone();
                file_created_this_op = true;
            } else {
                return Err(WorkspaceError::Io(format!(
                    "File {} not found, and the first/only operation's old_string is not empty (required for creation).",
                    file_path.display()
                )));
            }
        }
        Err(e) => {
            return Err(WorkspaceError::Io(format!(
                "Failed to read file {}: {e}",
                file_path.display()
            )));
        }
    }

    if operations.is_empty() {
        return Ok((current_content, 0, false));
    }

    let mut edits_applied_count = 0usize;
    for (index, edit_op) in operations.iter().enumerate() {
        if token.is_some_and(|t| t.is_cancelled()) {
            return Err(WorkspaceError::ToolExecution(
                "Operation cancelled".to_string(),
            ));
        }

        if edit_op.old_string.is_empty() {
            if index == 0 && file_created_this_op {
                // creation step
            } else if index == 0 && operations.len() == 1 {
                current_content = edit_op.new_string.clone();
                if !file_created_this_op {
                    file_created_this_op = true;
                }
            } else {
                return Err(WorkspaceError::ToolExecution(format!(
                    "Edit #{} for file {} has an empty old_string. This is only allowed for the first operation if the file is being created or for a single operation to overwrite the file.",
                    index + 1,
                    file_path.display()
                )));
            }
        } else {
            let occurrences = current_content.matches(&edit_op.old_string).count();
            if occurrences == 0 {
                return Err(WorkspaceError::ToolExecution(format!(
                    "For edit #{}, string not found in file {} (after {} previous successful edits). String to find (first 50 chars): '{}'",
                    index + 1,
                    file_path.display(),
                    edits_applied_count,
                    edit_op.old_string.chars().take(50).collect::<String>()
                )));
            }
            if occurrences > 1 {
                return Err(WorkspaceError::ToolExecution(format!(
                    "For edit #{}, found {} occurrences of string in file {} (after {} previous successful edits). String to find (first 50 chars): '{}'. Please provide more context.",
                    index + 1,
                    occurrences,
                    file_path.display(),
                    edits_applied_count,
                    edit_op.old_string.chars().take(50).collect::<String>()
                )));
            }
            current_content = current_content.replace(&edit_op.old_string, &edit_op.new_string);
        }
        edits_applied_count += 1;
    }

    Ok((current_content, edits_applied_count, file_created_this_op))
}

impl LocalWorkspace {
    pub async fn with_path(path: PathBuf) -> WorkspaceResult<Self> {
        let metadata = WorkspaceMetadata {
            id: format!("local:{}", path.display()),
            workspace_type: WorkspaceType::Local,
            location: path.display().to_string(),
        };

        Ok(Self {
            path,
            environment_cache: Arc::new(RwLock::new(None)),
            metadata,
        })
    }

    /// Collect environment information for the local workspace
    async fn collect_environment(&self) -> WorkspaceResult<EnvironmentInfo> {
        EnvironmentInfo::collect_for_path(&self.path)
    }
}

impl std::fmt::Debug for LocalWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalWorkspace")
            .field("path", &self.path)
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Workspace for LocalWorkspace {
    async fn environment(&self) -> WorkspaceResult<EnvironmentInfo> {
        let mut cache = self.environment_cache.write().await;

        // Check if we have valid cached data
        if let Some(cached) = cache.as_ref()
            && !cached.is_expired()
        {
            return Ok(cached.info.clone());
        }

        // Collect fresh environment info
        let env_info = self.collect_environment().await?;

        // Cache it with 5 minute TTL
        *cache = Some(CachedEnvironment::new(
            env_info.clone(),
            Duration::from_secs(300), // 5 minutes
        ));

        Ok(env_info)
    }

    fn metadata(&self) -> WorkspaceMetadata {
        self.metadata.clone()
    }

    async fn invalidate_environment_cache(&self) {
        let mut cache = self.environment_cache.write().await;
        *cache = None;
    }

    async fn list_files(
        &self,
        query: Option<&str>,
        max_results: Option<usize>,
    ) -> WorkspaceResult<Vec<String>> {
        use crate::utils::FileListingUtils;

        info!(target: "workspace.list_files", "Listing files in workspace: {:?}", self.path);

        FileListingUtils::list_files(&self.path, query, max_results).map_err(WorkspaceError::from)
    }

    fn working_directory(&self) -> &std::path::Path {
        &self.path
    }

    async fn read_file(
        &self,
        request: ReadFileRequest,
        ctx: &WorkspaceOpContext,
    ) -> WorkspaceResult<FileContentResult> {
        let abs_path = resolve_path(&self.path, &request.file_path);
        view_file_internal(
            &abs_path,
            request.offset,
            request.limit,
            &ctx.cancellation_token,
        )
        .await
        .map_err(|e| WorkspaceError::Io(e.to_string()))
    }

    async fn list_directory(
        &self,
        request: ListDirectoryRequest,
        ctx: &WorkspaceOpContext,
    ) -> WorkspaceResult<FileListResult> {
        let target_path = resolve_path(&self.path, &request.path);
        let target_path_str = target_path.to_string_lossy().to_string();
        let ignore_patterns = request.ignore.unwrap_or_default();
        let cancellation_token = ctx.cancellation_token.clone();

        let result = task::spawn_blocking(move || {
            list_directory_internal(&target_path_str, &ignore_patterns, &cancellation_token)
        })
        .await;

        match result {
            Ok(listing_result) => listing_result.map_err(|e| WorkspaceError::Io(e.to_string())),
            Err(join_error) => Err(WorkspaceError::Io(format!("Task join error: {join_error}"))),
        }
    }

    async fn glob(
        &self,
        request: GlobRequest,
        ctx: &WorkspaceOpContext,
    ) -> WorkspaceResult<GlobResult> {
        if ctx.cancellation_token.is_cancelled() {
            return Err(WorkspaceError::ToolExecution(
                "Operation cancelled".to_string(),
            ));
        }

        let search_path = request.path.as_deref().unwrap_or(".");
        let base_path = resolve_path(&self.path, search_path);

        let glob_pattern = format!("{}/{}", base_path.display(), request.pattern);

        let mut results = Vec::new();
        match glob::glob(&glob_pattern) {
            Ok(paths) => {
                for entry in paths {
                    if ctx.cancellation_token.is_cancelled() {
                        return Err(WorkspaceError::ToolExecution(
                            "Operation cancelled".to_string(),
                        ));
                    }

                    match entry {
                        Ok(path) => results.push(path.display().to_string()),
                        Err(e) => {
                            return Err(WorkspaceError::ToolExecution(format!(
                                "Error matching glob pattern '{glob_pattern}': {e}"
                            )));
                        }
                    }
                }
            }
            Err(e) => {
                return Err(WorkspaceError::ToolExecution(format!(
                    "Invalid glob pattern '{glob_pattern}': {e}"
                )));
            }
        }

        results.sort();
        Ok(GlobResult {
            matches: results,
            pattern: request.pattern,
        })
    }

    async fn grep(
        &self,
        request: GrepRequest,
        ctx: &WorkspaceOpContext,
    ) -> WorkspaceResult<SearchResult> {
        let search_path = request.path.as_deref().unwrap_or(".");
        let base_path = resolve_path(&self.path, search_path);

        let pattern = request.pattern.clone();
        let include = request.include.clone();
        let cancellation_token = ctx.cancellation_token.clone();

        let result = task::spawn_blocking(move || {
            grep_search_internal(
                &pattern,
                include.as_deref(),
                &base_path,
                &cancellation_token,
            )
        })
        .await;

        match result {
            Ok(search_result) => search_result.map_err(WorkspaceError::ToolExecution),
            Err(e) => Err(WorkspaceError::ToolExecution(format!(
                "Task join error: {e}"
            ))),
        }
    }

    async fn astgrep(
        &self,
        request: AstGrepRequest,
        ctx: &WorkspaceOpContext,
    ) -> WorkspaceResult<SearchResult> {
        let search_path = request.path.as_deref().unwrap_or(".");
        let base_path = resolve_path(&self.path, search_path);

        let pattern = request.pattern.clone();
        let lang = request.lang.clone();
        let include = request.include.clone();
        let exclude = request.exclude.clone();
        let cancellation_token = ctx.cancellation_token.clone();

        let result = task::spawn_blocking(move || {
            astgrep_search_internal(
                &pattern,
                lang.as_deref(),
                include.as_deref(),
                exclude.as_deref(),
                &base_path,
                &cancellation_token,
            )
        })
        .await;

        match result {
            Ok(search_result) => search_result.map_err(WorkspaceError::ToolExecution),
            Err(e) => Err(WorkspaceError::ToolExecution(format!(
                "Task join error: {e}"
            ))),
        }
    }

    async fn apply_edits(
        &self,
        request: ApplyEditsRequest,
        ctx: &WorkspaceOpContext,
    ) -> WorkspaceResult<EditResult> {
        let abs_path = resolve_path(&self.path, &request.file_path);
        let abs_path_str = abs_path.display().to_string();
        let file_lock = get_file_lock(&abs_path_str).await;
        let _lock_guard = file_lock.lock().await;

        let (final_content, num_ops, created_or_overwritten) =
            perform_edit_operations(&abs_path, &request.edits, Some(&ctx.cancellation_token))
                .await?;

        if created_or_overwritten || num_ops > 0 {
            if ctx.cancellation_token.is_cancelled() {
                return Err(WorkspaceError::ToolExecution(
                    "Operation cancelled".to_string(),
                ));
            }
            tokio::fs::write(&abs_path, &final_content)
                .await
                .map_err(|e| {
                    WorkspaceError::Io(format!(
                        "Failed to write file {}: {}",
                        abs_path.display(),
                        e
                    ))
                })?;

            Ok(EditResult {
                file_path: abs_path_str,
                changes_made: num_ops,
                file_created: created_or_overwritten,
                old_content: None,
                new_content: Some(final_content),
            })
        } else {
            Ok(EditResult {
                file_path: abs_path_str,
                changes_made: 0,
                file_created: false,
                old_content: None,
                new_content: None,
            })
        }
    }

    async fn write_file(
        &self,
        request: WriteFileRequest,
        ctx: &WorkspaceOpContext,
    ) -> WorkspaceResult<EditResult> {
        let abs_path = resolve_path(&self.path, &request.file_path);
        let abs_path_str = abs_path.display().to_string();
        let file_lock = get_file_lock(&abs_path_str).await;
        let _lock_guard = file_lock.lock().await;

        if ctx.cancellation_token.is_cancelled() {
            return Err(WorkspaceError::ToolExecution(
                "Operation cancelled".to_string(),
            ));
        }

        if let Some(parent) = abs_path.parent()
            && !parent.exists()
        {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                WorkspaceError::Io(format!(
                    "Failed to create parent directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let file_existed = abs_path.exists();
        tokio::fs::write(&abs_path, &request.content)
            .await
            .map_err(|e| {
                WorkspaceError::Io(format!("Failed to write file {}: {e}", abs_path.display()))
            })?;

        Ok(EditResult {
            file_path: abs_path_str,
            changes_made: 1,
            file_created: !file_existed,
            old_content: None,
            new_content: Some(request.content),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn test_local_workspace_creation() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();
        assert!(matches!(
            workspace.metadata().workspace_type,
            WorkspaceType::Local
        ));
    }

    #[tokio::test]
    async fn test_local_workspace_with_path() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        assert!(matches!(
            workspace.metadata().workspace_type,
            WorkspaceType::Local
        ));
        assert_eq!(
            workspace.metadata().location,
            temp_dir.path().display().to_string()
        );
    }

    #[tokio::test]
    async fn test_environment_caching() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // First call should collect fresh data
        let env1 = workspace.environment().await.unwrap();

        // Second call should return cached data
        let env2 = workspace.environment().await.unwrap();

        // Should be identical
        assert_eq!(env1.working_directory, env2.working_directory);
        assert_eq!(env1.vcs.is_some(), env2.vcs.is_some());
        assert_eq!(env1.platform, env2.platform);
    }

    #[tokio::test]
    async fn test_cache_invalidation() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // Get initial environment
        let _ = workspace.environment().await.unwrap();

        // Invalidate cache
        workspace.invalidate_environment_cache().await;

        // Should work fine and fetch fresh data
        let env = workspace.environment().await.unwrap();
        assert!(!env.working_directory.as_os_str().is_empty());
    }

    #[tokio::test]
    async fn test_environment_collection() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        let env = workspace.environment().await.unwrap();

        // Verify basic environment info
        let expected_path = temp_dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());

        // Canonicalize both paths for comparison on macOS
        let actual_canonical = env
            .working_directory
            .canonicalize()
            .unwrap_or(env.working_directory.clone());
        let expected_canonical = expected_path
            .canonicalize()
            .unwrap_or(expected_path.clone());

        assert_eq!(actual_canonical, expected_canonical);
    }

    #[tokio::test]
    async fn test_list_files() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // Create some test files
        std::fs::write(temp_dir.path().join("test.rs"), "test").unwrap();
        std::fs::write(temp_dir.path().join("main.rs"), "main").unwrap();
        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        std::fs::write(temp_dir.path().join("src/lib.rs"), "lib").unwrap();

        // List all files
        let files = workspace.list_files(None, None).await.unwrap();
        assert_eq!(files.len(), 4); // 3 files + 1 directory
        assert!(files.contains(&"test.rs".to_string()));
        assert!(files.contains(&"main.rs".to_string()));
        assert!(files.contains(&"src/".to_string())); // Directory with trailing slash
        assert!(files.contains(&"src/lib.rs".to_string()));

        // Test with query
        let files = workspace.list_files(Some("test"), None).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], "test.rs");

        // Test with max_results
        let files = workspace.list_files(None, Some(2)).await.unwrap();
        assert_eq!(files.len(), 2);
    }

    #[tokio::test]
    async fn test_list_files_includes_dotfiles() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        // Create a dotfile
        std::fs::write(temp_dir.path().join(".gitignore"), "target/").unwrap();

        let files = workspace.list_files(None, None).await.unwrap();
        assert!(files.contains(&".gitignore".to_string()));
    }

    #[tokio::test]
    async fn test_working_directory() {
        let temp_dir = tempdir().unwrap();
        let workspace = LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap();

        assert_eq!(workspace.working_directory(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_grep_orders_matches_by_mtime_then_path() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        let b_file = root.join("b.rs");
        let a_file = root.join("a.rs");

        std::fs::write(&b_file, "needle from b\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&a_file, "needle from a\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Refresh b so it has the newest mtime and should appear first.
        std::fs::write(&b_file, "needle from b updated\n").unwrap();

        let workspace = LocalWorkspace::with_path(root.to_path_buf()).await.unwrap();

        let context = WorkspaceOpContext::new("test-grep-order", CancellationToken::new());
        let result = workspace
            .grep(
                GrepRequest {
                    pattern: "needle".to_string(),
                    include: Some("*.rs".to_string()),
                    path: Some(".".to_string()),
                },
                &context,
            )
            .await
            .unwrap();

        assert!(result.search_completed);
        assert_eq!(result.total_files_searched, 2);
        assert_eq!(result.matches.len(), 2);

        let first = std::path::Path::new(&result.matches[0].file_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let second = std::path::Path::new(&result.matches[1].file_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        assert_eq!(first, "b.rs");
        assert_eq!(second, "a.rs");
    }

    #[tokio::test]
    async fn test_grep_include_filters_files() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();

        std::fs::write(root.join("src/lib.rs"), "needle in rust\n").unwrap();
        std::fs::write(root.join("src/readme.txt"), "needle in text\n").unwrap();
        std::fs::write(root.join("docs/guide.md"), "needle in markdown\n").unwrap();

        let workspace = LocalWorkspace::with_path(root.to_path_buf()).await.unwrap();
        let context = WorkspaceOpContext::new("test-grep-include", CancellationToken::new());
        let result = workspace
            .grep(
                GrepRequest {
                    pattern: "needle".to_string(),
                    include: Some("*.rs".to_string()),
                    path: Some(".".to_string()),
                },
                &context,
            )
            .await
            .unwrap();

        assert!(result.search_completed);
        assert_eq!(result.total_files_searched, 1);
        assert_eq!(result.matches.len(), 1);

        let file_name = std::path::Path::new(&result.matches[0].file_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(file_name, "lib.rs");
    }

    #[tokio::test]
    async fn test_grep_pre_cancelled_returns_incomplete_result() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        std::fs::write(root.join("a.rs"), "needle\n").unwrap();
        std::fs::write(root.join("b.rs"), "needle\n").unwrap();

        let workspace = LocalWorkspace::with_path(root.to_path_buf()).await.unwrap();
        let cancellation_token = CancellationToken::new();
        cancellation_token.cancel();
        let context = WorkspaceOpContext::new("test-grep-cancelled", cancellation_token);

        let result = workspace
            .grep(
                GrepRequest {
                    pattern: "needle".to_string(),
                    include: Some("*.rs".to_string()),
                    path: Some(".".to_string()),
                },
                &context,
            )
            .await
            .unwrap();

        assert!(!result.search_completed);
        assert_eq!(result.total_files_searched, 0);
        assert!(result.matches.is_empty());
    }
}
