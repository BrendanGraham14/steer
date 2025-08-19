use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use steer_tools::tools::workspace_tools;
use steer_tools::traits::ExecutableTool;
use steer_tools::{ExecutionContext, ToolError};
use steer_workspace::utils::{
    DirectoryStructureUtils, EnvironmentUtils, FileListingUtils, GitStatusUtils,
};
use steer_workspace::{MAX_DIRECTORY_DEPTH, MAX_DIRECTORY_ITEMS};

use crate::proto::{
    ExecuteToolRequest, ExecuteToolResponse, GetAgentInfoRequest, GetAgentInfoResponse,
    GetToolApprovalRequirementsRequest, GetToolApprovalRequirementsResponse, GetToolSchemasRequest,
    GetToolSchemasResponse, HealthRequest, HealthResponse, HealthStatus, ListFilesRequest,
    ListFilesResponse, ToolSchema as GrpcToolSchema, execute_tool_response::Result as ProtoResult,
    remote_workspace_service_server::RemoteWorkspaceService as RemoteWorkspaceServiceServer,
};
use steer_proto::common::v1::{
    BashResult as ProtoBashResult, ColumnRange as ProtoColumnRange, EditResult as ProtoEditResult,
    FileContentResult as ProtoFileContentResult, FileEntry as ProtoFileEntry,
    FileListResult as ProtoFileListResult, GlobResult as ProtoGlobResult,
    SearchMatch as ProtoSearchMatch, SearchResult as ProtoSearchResult,
    TodoListResult as ProtoTodoListResult, TodoWriteResult as ProtoTodoWriteResult,
};

use steer_grpc::grpc::conversions::{
    convert_todo_item_to_proto, convert_todo_write_file_operation_to_proto,
};

/// Agent service implementation that executes tools locally
///
/// This service receives tool execution requests via gRPC and executes them
/// using the standard tool executor. It's designed to run on remote machines,
/// VMs, or containers to provide remote tool execution capabilities.
pub struct RemoteWorkspaceService {
    working_dir: PathBuf,
    tools: Arc<HashMap<String, Box<dyn ExecutableTool>>>,
    version: String,
}

impl RemoteWorkspaceService {
    /// Create a new RemoteWorkspaceService with the standard tool set
    pub fn new(working_dir: PathBuf) -> Result<Self, ToolError> {
        Self::with_tools(workspace_tools(), working_dir)
    }

    /// Create a new RemoteWorkspaceService with a custom set of tools
    pub fn with_tools(
        tools_list: Vec<Box<dyn ExecutableTool>>,
        working_dir: PathBuf,
    ) -> Result<Self, ToolError> {
        let mut tools: HashMap<String, Box<dyn ExecutableTool>> = HashMap::new();

        // Register the provided tools
        for tool in tools_list {
            tools.insert(tool.name().to_string(), tool);
        }

        Ok(Self {
            working_dir,
            tools: Arc::new(tools),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    /// Get the supported tools from the tool executor
    pub fn get_supported_tools(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Convert a ToolError to a ToolErrorDetail payload
    fn tool_error_to_detail(error: &ToolError) -> crate::proto::ToolErrorDetail {
        use crate::proto::tool_error_detail::Kind;

        let (kind, tool_name, message) = match error {
            ToolError::Execution { tool_name, message } => {
                (Kind::Execution, tool_name.clone(), message.clone())
            }
            ToolError::Io { tool_name, message } => (Kind::Io, tool_name.clone(), message.clone()),
            ToolError::InvalidParams(tool_name, message) => {
                (Kind::InvalidParams, tool_name.clone(), message.clone())
            }
            ToolError::Cancelled(tool_name) => (
                Kind::Cancelled,
                tool_name.clone(),
                format!("{tool_name} was cancelled"),
            ),
            ToolError::Timeout(tool_name) => (
                Kind::Timeout,
                tool_name.clone(),
                format!("{tool_name} execution timed out"),
            ),
            ToolError::UnknownTool(tool_name) => (
                Kind::UnknownTool,
                tool_name.clone(),
                format!("Unknown tool: {tool_name}"),
            ),
            ToolError::DeniedByUser(tool_name) => (
                Kind::DeniedByUser,
                tool_name.clone(),
                format!("Tool execution denied: {tool_name}"),
            ),
            ToolError::InternalError(msg) => (Kind::Internal, String::new(), msg.clone()),
        };

        crate::proto::ToolErrorDetail {
            kind: kind as i32,
            tool_name,
            message,
        }
    }

    /// Convert a ToolError to a gRPC Status
    fn tool_error_to_status(error: ToolError) -> Status {
        match error {
            ToolError::Cancelled(_) => Status::cancelled("Tool execution was cancelled"),
            ToolError::UnknownTool(tool_name) => {
                Status::not_found(format!("Unknown tool: {tool_name}"))
            }
            ToolError::InvalidParams(tool_name, message) => Status::invalid_argument(format!(
                "Invalid parameters for tool {tool_name}: {message}"
            )),
            ToolError::Execution { tool_name, message } => {
                Status::internal(format!("Tool {tool_name} execution failed: {message}"))
            }
            ToolError::Io { tool_name, message } => {
                Status::internal(format!("IO error in tool {tool_name}: {message}"))
            }
            ToolError::DeniedByUser(tool_name) => {
                Status::permission_denied(format!("Tool execution denied: {tool_name}"))
            }
            ToolError::Timeout(tool_name) => {
                Status::deadline_exceeded(format!("Tool {tool_name} execution timed out"))
            }
            ToolError::InternalError(message) => {
                Status::internal(format!("Internal error: {message}"))
            }
        }
    }

    /// Convert a typed tool output to a proto result
    fn tool_result_to_proto_result(
        result: &steer_tools::result::ToolResult,
    ) -> Option<ProtoResult> {
        // Match on the ToolResult enum variants
        match result {
            steer_tools::result::ToolResult::Search(search_result) => {
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

            steer_tools::result::ToolResult::FileList(file_list) => {
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

            steer_tools::result::ToolResult::FileContent(file_content) => {
                Some(ProtoResult::FileContentResult(ProtoFileContentResult {
                    content: file_content.content.clone(),
                    file_path: file_content.file_path.clone(),
                    line_count: file_content.line_count as u64,
                    truncated: file_content.truncated,
                }))
            }

            steer_tools::result::ToolResult::Edit(edit_result) => {
                Some(ProtoResult::EditResult(ProtoEditResult {
                    file_path: edit_result.file_path.clone(),
                    changes_made: edit_result.changes_made as u64,
                    file_created: edit_result.file_created,
                    old_content: edit_result.old_content.clone(),
                    new_content: edit_result.new_content.clone(),
                }))
            }

            steer_tools::result::ToolResult::Bash(bash_result) => {
                Some(ProtoResult::BashResult(ProtoBashResult {
                    stdout: bash_result.stdout.clone(),
                    stderr: bash_result.stderr.clone(),
                    exit_code: bash_result.exit_code,
                    command: bash_result.command.clone(),
                }))
            }

            steer_tools::result::ToolResult::Glob(glob_result) => {
                Some(ProtoResult::GlobResult(ProtoGlobResult {
                    matches: glob_result.matches.clone(),
                    pattern: glob_result.pattern.clone(),
                }))
            }

            steer_tools::result::ToolResult::TodoRead(todo_list) => {
                let proto_todos = todo_list
                    .todos
                    .iter()
                    .map(convert_todo_item_to_proto)
                    .collect();

                Some(ProtoResult::TodoListResult(ProtoTodoListResult {
                    todos: proto_todos,
                }))
            }

            steer_tools::result::ToolResult::TodoWrite(todo_write_result) => {
                let proto_todos = todo_write_result
                    .todos
                    .iter()
                    .map(convert_todo_item_to_proto)
                    .collect();

                Some(ProtoResult::TodoWriteResult(ProtoTodoWriteResult {
                    todos: proto_todos,
                    operation: convert_todo_write_file_operation_to_proto(
                        &todo_write_result.operation,
                    ) as i32,
                }))
            }

            steer_tools::result::ToolResult::Fetch(_) => {
                // Fetch results are not handled in the remote workspace
                None
            }

            steer_tools::result::ToolResult::Agent(_) => {
                // Agent results are not handled in the remote workspace
                None
            }

            steer_tools::result::ToolResult::External(_) => {
                // External results are not handled in the remote workspace
                None
            }

            steer_tools::result::ToolResult::Error(_) => {
                // Errors are handled differently
                None
            }
        }
    }

    /// Get directory structure for environment info
    fn get_directory_structure(&self) -> Result<String, std::io::Error> {
        DirectoryStructureUtils::get_directory_structure(
            &self.working_dir,
            MAX_DIRECTORY_DEPTH,
            Some(MAX_DIRECTORY_ITEMS),
        )
    }

    /// Get git status information
    async fn get_git_status(&self) -> Result<String, std::io::Error> {
        GitStatusUtils::get_git_status(&self.working_dir)
    }
}

#[tonic::async_trait]
impl RemoteWorkspaceServiceServer for RemoteWorkspaceService {
    type ListFilesStream = ReceiverStream<Result<ListFilesResponse, Status>>;
    /// Get tool schemas
    async fn get_tool_schemas(
        &self,
        _request: Request<GetToolSchemasRequest>,
    ) -> Result<Response<GetToolSchemasResponse>, Status> {
        let mut schemas = Vec::new();

        for (name, tool) in self.tools.iter() {
            let input_schema = tool.input_schema();
            let input_schema_json = serde_json::to_string(&input_schema)
                .map_err(|e| Status::internal(format!("Failed to serialize schema: {e}")))?;

            schemas.push(GrpcToolSchema {
                name: name.clone(),
                description: tool.description(),
                input_schema_json,
            });
        }

        Ok(Response::new(GetToolSchemasResponse { tools: schemas }))
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
                Status::invalid_argument(format!("Failed to parse tool parameters: {e}"))
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
                    error_detail: None,
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
                        error_detail: Some(Self::tool_error_to_detail(&error)),
                    },
                }
            }
        };

        Ok(Response::new(response))
    }

    /// Get information about the agent and available tools
    async fn get_agent_info(
        &self,
        _request: Request<GetAgentInfoRequest>,
    ) -> Result<Response<GetAgentInfoResponse>, Status> {
        let supported_tools = self.get_supported_tools();

        let info = GetAgentInfoResponse {
            version: self.version.clone(),
            supported_tools,
            metadata: std::collections::HashMap::from([
                (
                    "hostname".to_string(),
                    gethostname::gethostname().to_string_lossy().to_string(),
                ),
                (
                    "working_directory".to_string(),
                    self.working_dir.to_string_lossy().to_string(),
                ),
            ]),
        };

        Ok(Response::new(info))
    }

    /// Health check
    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
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
        request: Request<GetToolApprovalRequirementsRequest>,
    ) -> Result<Response<GetToolApprovalRequirementsResponse>, Status> {
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

        Ok(Response::new(GetToolApprovalRequirementsResponse {
            approval_requirements,
        }))
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
            self.working_dir.to_string_lossy().to_string()
        };

        // Check if it's a git repo
        let is_git_repo = EnvironmentUtils::is_git_repo(Path::new(&working_directory));

        // Get platform information
        let platform = EnvironmentUtils::get_platform().to_string();

        // Get current date
        let date = EnvironmentUtils::get_current_date();

        // Get directory structure (simplified for now)
        let directory_structure = self.get_directory_structure().unwrap_or_else(|_| {
            format!("Failed to read directory structure from {working_directory}")
        });

        // Get git status if it's a git repo
        let git_status = if is_git_repo {
            self.get_git_status().await.ok()
        } else {
            None
        };

        // Read README.md if it exists
        let readme_content = EnvironmentUtils::read_readme(Path::new(&working_directory));

        // Read CLAUDE.md if it exists
        let claude_md_content = EnvironmentUtils::read_claude_md(Path::new(&working_directory));

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
        let req = request.into_inner();

        // Create the response stream
        let (tx, rx) = mpsc::channel(100);

        // Spawn task to stream the files
        tokio::spawn(async move {
            // Use the shared file listing utility
            let query = if req.query.is_empty() {
                None
            } else {
                Some(req.query.as_str())
            };
            let max_results = if req.max_results > 0 {
                Some(req.max_results as usize)
            } else {
                None
            };

            let files = match FileListingUtils::list_files(Path::new("."), query, max_results) {
                Ok(files) => files,
                Err(e) => {
                    tracing::error!("Error listing files: {}", e);
                    return;
                }
            };

            // Stream files in chunks of 1000
            for chunk in files.chunks(1000) {
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
