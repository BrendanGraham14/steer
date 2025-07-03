use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use conductor_tools::tools::workspace_tools;
use conductor_tools::traits::ExecutableTool;
use conductor_tools::{ExecutionContext, ToolError};

use crate::proto::{
    AgentInfo, BashResult as ProtoBashResult, ColumnRange as ProtoColumnRange,
    EditResult as ProtoEditResult, ExecuteToolRequest, ExecuteToolResponse,
    FileContentResult as ProtoFileContentResult, FileEntry as ProtoFileEntry,
    FileListResult as ProtoFileListResult, GlobResult as ProtoGlobResult, HealthResponse,
    HealthStatus, ListFilesRequest, ListFilesResponse, SearchMatch as ProtoSearchMatch,
    SearchResult as ProtoSearchResult, TodoItem as ProtoTodoItem,
    TodoListResult as ProtoTodoListResult, TodoWriteResult as ProtoTodoWriteResult,
    ToolSchema as GrpcToolSchema, ToolSchemasResponse,
    execute_tool_response::Result as ProtoResult,
    remote_workspace_service_server::RemoteWorkspaceService as RemoteWorkspaceServiceServer,
};

/// Agent service implementation that executes tools locally
///
/// This service receives tool execution requests via gRPC and executes them
/// using the standard tool executor. It's designed to run on remote machines,
/// VMs, or containers to provide remote tool execution capabilities.
pub struct RemoteWorkspaceService {
    tools: Arc<HashMap<String, Box<dyn ExecutableTool>>>,
    version: String,
}

impl RemoteWorkspaceService {
    /// Create a new RemoteWorkspaceService with the standard tool set
    pub fn new() -> Result<Self, ToolError> {
        Self::with_tools(workspace_tools())
    }

    /// Create a new RemoteWorkspaceService with a custom set of tools
    pub fn with_tools(tools_list: Vec<Box<dyn ExecutableTool>>) -> Result<Self, ToolError> {
        let mut tools: HashMap<String, Box<dyn ExecutableTool>> = HashMap::new();

        // Register the provided tools
        for tool in tools_list {
            tools.insert(tool.name().to_string(), tool);
        }

        Ok(Self {
            tools: Arc::new(tools),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    /// Get the supported tools from the tool executor
    pub fn get_supported_tools(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Convert a ToolError to a gRPC Status
    fn tool_error_to_status(error: ToolError) -> Status {
        match error {
            ToolError::Cancelled(_) => Status::cancelled("Tool execution was cancelled"),
            ToolError::UnknownTool(tool_name) => {
                Status::not_found(format!("Unknown tool: {}", tool_name))
            }
            ToolError::InvalidParams(tool_name, message) => Status::invalid_argument(format!(
                "Invalid parameters for tool {}: {}",
                tool_name, message
            )),
            ToolError::Execution { tool_name, message } => {
                Status::internal(format!("Tool {} execution failed: {}", tool_name, message))
            }
            ToolError::Io { tool_name, message } => {
                Status::internal(format!("IO error in tool {}: {}", tool_name, message))
            }
            ToolError::DeniedByUser(tool_name) => {
                Status::permission_denied(format!("Tool execution denied: {}", tool_name))
            }
            ToolError::Timeout(tool_name) => {
                Status::deadline_exceeded(format!("Tool {} execution timed out", tool_name))
            }
            ToolError::InternalError(message) => {
                Status::internal(format!("Internal error: {}", message))
            }
            ToolError::Serialization(e) => Status::internal(format!("Serialization error: {}", e)),
            ToolError::Http(e) => Status::internal(format!("HTTP error: {}", e)),
            ToolError::Regex(e) => Status::internal(format!("Regex error: {}", e)),
            ToolError::McpConnectionFailed {
                server_name,
                message,
            } => Status::internal(format!(
                "MCP server {} connection failed: {}",
                server_name, message
            )),
        }
    }

    /// Convert a typed tool output to a proto result
    fn tool_result_to_proto_result(
        result: &conductor_tools::result::ToolResult,
    ) -> Option<ProtoResult> {
        // Match on the ToolResult enum variants
        match result {
            conductor_tools::result::ToolResult::Search(search_result) => {
                let proto_matches = search_result
                    .matches
                    .iter()
                    .map(|m| ProtoSearchMatch {
                        file_path: m.file_path.clone(),
                        line_number: m.line_number as u64,
                        line_content: m.line_content.clone(),
                        column_range: m.column_range.map(|(start, end)| ProtoColumnRange {
                            start: start as u64,
                            end: end as u64,
                        }),
                    })
                    .collect();

                Some(ProtoResult::SearchResult(ProtoSearchResult {
                    matches: proto_matches,
                    total_files_searched: search_result.total_files_searched as u64,
                    search_completed: search_result.search_completed,
                }))
            }

            conductor_tools::result::ToolResult::FileList(file_list) => {
                let proto_entries = file_list
                    .entries
                    .iter()
                    .map(|e| ProtoFileEntry {
                        path: e.path.clone(),
                        is_directory: e.is_directory,
                        size: e.size,
                        permissions: e.permissions.clone(),
                    })
                    .collect();

                Some(ProtoResult::FileListResult(ProtoFileListResult {
                    entries: proto_entries,
                    base_path: file_list.base_path.clone(),
                }))
            }

            conductor_tools::result::ToolResult::FileContent(file_content) => {
                Some(ProtoResult::FileContentResult(ProtoFileContentResult {
                    content: file_content.content.clone(),
                    file_path: file_content.file_path.clone(),
                    line_count: file_content.line_count as u64,
                    truncated: file_content.truncated,
                }))
            }

            conductor_tools::result::ToolResult::Edit(edit_result) => {
                Some(ProtoResult::EditResult(ProtoEditResult {
                    file_path: edit_result.file_path.clone(),
                    changes_made: edit_result.changes_made as u64,
                    file_created: edit_result.file_created,
                    old_content: edit_result.old_content.clone(),
                    new_content: edit_result.new_content.clone(),
                }))
            }

            conductor_tools::result::ToolResult::Bash(bash_result) => {
                Some(ProtoResult::BashResult(ProtoBashResult {
                    stdout: bash_result.stdout.clone(),
                    stderr: bash_result.stderr.clone(),
                    exit_code: bash_result.exit_code,
                    command: bash_result.command.clone(),
                }))
            }

            conductor_tools::result::ToolResult::Glob(glob_result) => {
                Some(ProtoResult::GlobResult(ProtoGlobResult {
                    matches: glob_result.matches.clone(),
                    pattern: glob_result.pattern.clone(),
                }))
            }

            conductor_tools::result::ToolResult::TodoRead(todo_list) => {
                let proto_todos = todo_list
                    .todos
                    .iter()
                    .map(|t| ProtoTodoItem {
                        id: t.id.clone(),
                        content: t.content.clone(),
                        status: t.status.clone(),
                        priority: t.priority.clone(),
                    })
                    .collect();

                Some(ProtoResult::TodoListResult(ProtoTodoListResult {
                    todos: proto_todos,
                }))
            }

            conductor_tools::result::ToolResult::TodoWrite(todo_write_result) => {
                let proto_todos = todo_write_result
                    .todos
                    .iter()
                    .map(|t| ProtoTodoItem {
                        id: t.id.clone(),
                        content: t.content.clone(),
                        status: t.status.clone(),
                        priority: t.priority.clone(),
                    })
                    .collect();

                Some(ProtoResult::TodoWriteResult(ProtoTodoWriteResult {
                    todos: proto_todos,
                    operation: todo_write_result.operation.clone(),
                }))
            }

            conductor_tools::result::ToolResult::Fetch(_) => {
                // Fetch results are not handled in the remote workspace
                None
            }

            conductor_tools::result::ToolResult::Agent(_) => {
                // Agent results are not handled in the remote workspace
                None
            }

            conductor_tools::result::ToolResult::External(_) => {
                // External results are not handled in the remote workspace
                None
            }

            conductor_tools::result::ToolResult::Error(_) => {
                // Errors are handled differently
                None
            }
        }
    }

    /// Get directory structure for environment info
    fn get_directory_structure(&self) -> Result<String, std::io::Error> {
        let current_dir = std::env::current_dir()?;
        let mut structure = vec![current_dir.display().to_string()];

        // Simple directory traversal (limited depth to avoid huge responses)
        self.collect_directory_paths(&current_dir, &mut structure, 0, 3)?;

        structure.sort();
        Ok(structure.join("\n"))
    }

    /// Recursively collect directory paths
    fn collect_directory_paths(
        &self,
        dir: &Path,
        paths: &mut Vec<String>,
        current_depth: usize,
        max_depth: usize,
    ) -> Result<(), std::io::Error> {
        if current_depth >= max_depth {
            return Ok(());
        }

        let entries = std::fs::read_dir(dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();

            // Skip hidden files and directories
            if file_name.starts_with('.') {
                continue;
            }

            if let Ok(rel_path) = path.strip_prefix(&std::env::current_dir()?) {
                let path_str = rel_path.to_string_lossy().to_string();
                if path.is_dir() {
                    paths.push(format!("{}/", path_str));
                    self.collect_directory_paths(&path, paths, current_depth + 1, max_depth)?;
                } else {
                    paths.push(path_str);
                }
            }
        }

        Ok(())
    }

    /// Get git status information
    async fn get_git_status(&self) -> Result<String, std::io::Error> {
        use git2::Repository;

        let mut result = String::new();

        let repo = Repository::discover(".").map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to open git repository: {}", e),
            )
        })?;

        // Get current branch
        match repo.head() {
            Ok(head) => {
                let branch = if head.is_branch() {
                    head.shorthand().unwrap_or("<unknown>")
                } else {
                    "HEAD (detached)"
                };
                result.push_str(&format!("Current branch: {}\n\n", branch));
            }
            Err(e) => {
                // Handle case where HEAD doesn't exist (new repo)
                if e.code() == git2::ErrorCode::UnbornBranch {
                    result.push_str("Current branch: <unborn>\n\n");
                } else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to get HEAD: {}", e),
                    ));
                }
            }
        }

        // Get status
        let statuses = repo.statuses(None).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to get git status: {}", e),
            )
        })?;
        result.push_str("Status:\n");
        if statuses.is_empty() {
            result.push_str("Working tree clean\n");
        } else {
            for entry in statuses.iter() {
                let status = entry.status();
                let path = entry.path().unwrap_or("<unknown>");

                let status_char = if status.contains(git2::Status::INDEX_NEW) {
                    "A"
                } else if status.contains(git2::Status::INDEX_MODIFIED) {
                    "M"
                } else if status.contains(git2::Status::INDEX_DELETED) {
                    "D"
                } else if status.contains(git2::Status::WT_NEW) {
                    "?"
                } else if status.contains(git2::Status::WT_MODIFIED) {
                    "M"
                } else if status.contains(git2::Status::WT_DELETED) {
                    "D"
                } else {
                    " "
                };

                let wt_char = if status.contains(git2::Status::WT_NEW) {
                    "?"
                } else if status.contains(git2::Status::WT_MODIFIED) {
                    "M"
                } else if status.contains(git2::Status::WT_DELETED) {
                    "D"
                } else {
                    " "
                };

                result.push_str(&format!("{}{} {}\n", status_char, wt_char, path));
            }
        }

        // Get recent commits
        result.push_str("\nRecent commits:\n");
        match repo.revwalk() {
            Ok(mut revwalk) => {
                if let Ok(()) = revwalk.push_head() {
                    let mut count = 0;
                    for oid in revwalk {
                        if count >= 5 {
                            break;
                        }
                        if let Ok(oid) = oid {
                            if let Ok(commit) = repo.find_commit(oid) {
                                let summary = commit.summary().unwrap_or("<no summary>");
                                let id = commit.id();
                                result.push_str(&format!("{:.7} {}\n", id, summary));
                                count += 1;
                            }
                        }
                    }
                    if count == 0 {
                        result.push_str("<no commits>\n");
                    }
                } else {
                    result.push_str("<no commits>\n");
                }
            }
            Err(_) => {
                result.push_str("<no commits>\n");
            }
        }

        Ok(result)
    }
}

#[tonic::async_trait]
impl RemoteWorkspaceServiceServer for RemoteWorkspaceService {
    type ListFilesStream = ReceiverStream<Result<ListFilesResponse, Status>>;
    /// Get tool schemas
    async fn get_tool_schemas(
        &self,
        _request: Request<()>,
    ) -> Result<Response<ToolSchemasResponse>, Status> {
        let mut schemas = Vec::new();

        for (name, tool) in self.tools.iter() {
            let input_schema = tool.input_schema();
            let input_schema_json = serde_json::to_string(&input_schema)
                .map_err(|e| Status::internal(format!("Failed to serialize schema: {}", e)))?;

            schemas.push(GrpcToolSchema {
                name: name.clone(),
                description: tool.description(),
                input_schema_json,
            });
        }

        Ok(Response::new(ToolSchemasResponse { tools: schemas }))
    }

    /// Execute a tool call on the agent
    async fn execute_tool(
        &self,
        request: Request<ExecuteToolRequest>,
    ) -> Result<Response<ExecuteToolResponse>, Status> {
        let start_time = std::time::Instant::now();
        let req = request.into_inner();

        // Parse the tool parameters
        let parameters: serde_json::Value =
            serde_json::from_str(&req.parameters_json).map_err(|e| {
                Status::invalid_argument(format!("Failed to parse tool parameters: {}", e))
            })?;

        // Look up the tool
        let tool = self
            .tools
            .get(&req.tool_name)
            .ok_or_else(|| Status::not_found(format!("Unknown tool: {}", req.tool_name)))?;

        // Create a cancellation token and a drop guard. When the gRPC request is cancelled,
        // this async function will be dropped, which triggers the drop guard to cancel the token.
        // This ensures that long-running tools (like bash commands) are properly cancelled.
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let _guard = cancellation_token.clone().drop_guard();

        // Create execution context
        let context = ExecutionContext::new(req.tool_call_id.clone())
            .with_cancellation_token(cancellation_token);

        let result = tool.run(parameters, &context).await;

        let end_time = std::time::Instant::now();
        let duration = end_time - start_time;

        // Convert result to response
        let response = match result {
            Ok(tool_result) => {
                // Convert to a typed result
                let proto_result = Self::tool_result_to_proto_result(&tool_result);

                ExecuteToolResponse {
                    success: true,
                    result: proto_result.or_else(|| {
                        // Fallback to string result
                        Some(ProtoResult::StringResult(tool_result.llm_format()))
                    }),
                    error: String::new(),
                    started_at: Some(prost_types::Timestamp {
                        seconds: start_time.elapsed().as_secs() as i64,
                        nanos: 0,
                    }),
                    completed_at: Some(prost_types::Timestamp {
                        seconds: duration.as_secs() as i64,
                        nanos: duration.subsec_nanos() as i32,
                    }),
                    metadata: std::collections::HashMap::new(),
                }
            }
            Err(error) => {
                // For some errors, we want to return them as successful responses
                // with the error in the error field, rather than failing the gRPC call
                match &error {
                    ToolError::Cancelled(_) => {
                        return Err(Status::cancelled("Tool execution was cancelled"));
                    }
                    ToolError::UnknownTool(_) => {
                        return Err(Self::tool_error_to_status(error));
                    }
                    _ => ExecuteToolResponse {
                        success: false,
                        result: None,
                        error: error.to_string(),
                        started_at: Some(prost_types::Timestamp {
                            seconds: start_time.elapsed().as_secs() as i64,
                            nanos: 0,
                        }),
                        completed_at: Some(prost_types::Timestamp {
                            seconds: duration.as_secs() as i64,
                            nanos: duration.subsec_nanos() as i32,
                        }),
                        metadata: std::collections::HashMap::new(),
                    },
                }
            }
        };

        Ok(Response::new(response))
    }

    /// Get information about the agent and available tools
    async fn get_agent_info(&self, _request: Request<()>) -> Result<Response<AgentInfo>, Status> {
        let supported_tools = self.get_supported_tools();

        let info = AgentInfo {
            version: self.version.clone(),
            supported_tools,
            metadata: std::collections::HashMap::from([
                (
                    "hostname".to_string(),
                    gethostname::gethostname().to_string_lossy().to_string(),
                ),
                (
                    "working_directory".to_string(),
                    std::env::current_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| "unknown".to_string()),
                ),
            ]),
        };

        Ok(Response::new(info))
    }

    /// Health check
    async fn health(&self, _request: Request<()>) -> Result<Response<HealthResponse>, Status> {
        // Simple health check - we could add more sophisticated checks here
        let response = HealthResponse {
            status: HealthStatus::Serving as i32,
            message: "Agent is healthy and ready to execute tools".to_string(),
            details: std::collections::HashMap::from([(
                "tool_count".to_string(),
                self.get_supported_tools().len().to_string(),
            )]),
        };

        Ok(Response::new(response))
    }

    /// Get tool approval requirements
    async fn get_tool_approval_requirements(
        &self,
        request: Request<crate::proto::ToolApprovalRequirementsRequest>,
    ) -> Result<Response<crate::proto::ToolApprovalRequirementsResponse>, Status> {
        let req = request.into_inner();
        let mut approval_requirements = std::collections::HashMap::new();

        for tool_name in req.tool_names {
            if let Some(tool) = self.tools.get(&tool_name) {
                approval_requirements.insert(tool_name, tool.requires_approval());
            } else {
                // Unknown tools are not included in the response
                // This matches the behavior of the local backend which returns UnknownTool error
            }
        }

        Ok(Response::new(
            crate::proto::ToolApprovalRequirementsResponse {
                approval_requirements,
            },
        ))
    }

    /// Get environment information for the remote workspace
    async fn get_environment_info(
        &self,
        request: Request<crate::proto::GetEnvironmentInfoRequest>,
    ) -> Result<Response<crate::proto::GetEnvironmentInfoResponse>, Status> {
        let req = request.into_inner();

        // Use the provided working directory or current directory
        let working_directory = if let Some(dir) = req.working_directory {
            dir
        } else {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .map_err(|e| Status::internal(format!("Failed to get current directory: {}", e)))?
        };

        // Change to the working directory if specified
        if working_directory != std::env::current_dir().unwrap().to_string_lossy() {
            if let Err(e) = std::env::set_current_dir(&working_directory) {
                return Err(Status::invalid_argument(format!(
                    "Failed to change to directory {}: {}",
                    working_directory, e
                )));
            }
        }

        // Check if it's a git repo
        let is_git_repo = git2::Repository::discover(&working_directory).is_ok();

        // Get platform information
        let platform = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "unknown"
        }
        .to_string();

        // Get current date (simplified)
        let date = "2025-06-17".to_string(); // TODO: Use proper date formatting

        // Get directory structure (simplified for now)
        let directory_structure = self.get_directory_structure().unwrap_or_else(|_| {
            format!(
                "Failed to read directory structure from {}",
                working_directory
            )
        });

        // Get git status if it's a git repo
        let git_status = if is_git_repo {
            self.get_git_status().await.ok()
        } else {
            None
        };

        // Read README.md if it exists
        let readme_content = std::fs::read_to_string("README.md").ok();

        // Read CLAUDE.md if it exists
        let claude_md_content = std::fs::read_to_string("CLAUDE.md").ok();

        let response = crate::proto::GetEnvironmentInfoResponse {
            working_directory,
            is_git_repo,
            platform,
            date,
            directory_structure,
            git_status,
            readme_content,
            claude_md_content,
        };

        Ok(Response::new(response))
    }

    /// List files in the workspace for fuzzy finding
    async fn list_files(
        &self,
        request: Request<ListFilesRequest>,
    ) -> Result<Response<Self::ListFilesStream>, Status> {
        use fuzzy_matcher::FuzzyMatcher;
        use fuzzy_matcher::skim::SkimMatcherV2;
        use ignore::Walk;

        let req = request.into_inner();

        // Create the response stream
        let (tx, rx) = mpsc::channel(100);

        // Spawn task to stream the files
        let list_task: JoinHandle<()> = tokio::spawn(async move {
            let mut files = Vec::new();

            // Walk the current directory, respecting .gitignore
            for entry in Walk::new(".") {
                match entry {
                    Ok(entry) => {
                        // Skip hidden files/directories (those starting with .)
                        if let Some(file_name) = entry.file_name().to_str() {
                            if file_name.starts_with('.') && entry.path() != Path::new(".") {
                                continue;
                            }
                        }

                        // Get the relative path from the root
                        if let Ok(relative_path) = entry.path().strip_prefix(".") {
                            if let Some(path_str) = relative_path.to_str() {
                                if !path_str.is_empty() {
                                    // Add trailing slash for directories
                                    if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                                        files.push(format!("{}/", path_str));
                                    } else {
                                        files.push(path_str.to_string());
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error walking directory: {}", e);
                    }
                }
            }

            // Apply fuzzy filter if query is provided
            let filtered_files = if !req.query.is_empty() {
                let matcher = SkimMatcherV2::default();
                let mut scored_files: Vec<(i64, String)> = files
                    .into_iter()
                    .filter_map(|file| {
                        matcher
                            .fuzzy_match(&file, &req.query)
                            .map(|score| (score, file))
                    })
                    .collect();

                // Sort by score (highest first)
                scored_files.sort_by(|a, b| b.0.cmp(&a.0));

                scored_files.into_iter().map(|(_, file)| file).collect()
            } else {
                files
            };

            // Apply max_results limit if specified
            let limited_files = if req.max_results > 0 {
                filtered_files
                    .into_iter()
                    .take(req.max_results as usize)
                    .collect()
            } else {
                filtered_files
            };

            // Stream files in chunks of 1000
            for chunk in limited_files.chunks(1000) {
                let response = ListFilesResponse {
                    paths: chunk.to_vec(),
                };

                // If the receiver is dropped (client cancelled), this will fail and exit the loop
                if let Err(e) = tx.send(Ok(response)).await {
                    tracing::debug!("Client cancelled file list stream: {}", e);
                    break;
                }
            }
        });

        // Note: The task handle is stored but not awaited here because the gRPC
        // streaming response will consume the receiver. The task will complete
        // when either all files are sent or the receiver is dropped (client cancellation).

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
