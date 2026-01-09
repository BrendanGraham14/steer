use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use steer_tools::tools::workspace_tools;
use steer_tools::traits::ExecutableTool;
use steer_tools::{ExecutionContext, ToolError};
use steer_workspace::utils::FileListingUtils;
use steer_workspace::{EnvironmentInfo, VcsInfo, VcsKind, VcsStatus};

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

use steer_grpc::grpc::{convert_todo_item_to_proto, convert_todo_write_file_operation_to_proto};

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
                format!("Tool execution denied by user: {tool_name}"),
            ),
            ToolError::DeniedByPolicy(tool_name) => (
                Kind::DeniedByPolicy,
                tool_name.clone(),
                format!("Tool execution denied by policy: {tool_name}"),
            ),
            ToolError::InternalError(msg) => (Kind::Internal, String::new(), msg.clone()),
        };

        crate::proto::ToolErrorDetail {
            kind: kind as i32,
            tool_name,
            message,
        }
    }

    fn convert_vcs_to_proto(info: VcsInfo) -> crate::proto::VcsInfo {
        let kind = match info.kind {
            VcsKind::Git => crate::proto::VcsKind::Git,
            VcsKind::Jj => crate::proto::VcsKind::Jj,
        };

        let status = match info.status {
            VcsStatus::Git(status) => {
                let head = status.head.map(|head| {
                    let (kind, branch) = match head {
                        steer_workspace::GitHead::Branch(branch) => {
                            (crate::proto::GitHeadKind::Branch, Some(branch))
                        }
                        steer_workspace::GitHead::Detached => {
                            (crate::proto::GitHeadKind::Detached, None)
                        }
                        steer_workspace::GitHead::Unborn => {
                            (crate::proto::GitHeadKind::Unborn, None)
                        }
                    };
                    crate::proto::GitHead {
                        kind: kind as i32,
                        branch,
                    }
                });

                let entries = status
                    .entries
                    .into_iter()
                    .map(|entry| crate::proto::GitStatusEntry {
                        summary: match entry.summary {
                            steer_workspace::GitStatusSummary::Added => {
                                crate::proto::GitStatusSummary::Added as i32
                            }
                            steer_workspace::GitStatusSummary::Removed => {
                                crate::proto::GitStatusSummary::Removed as i32
                            }
                            steer_workspace::GitStatusSummary::Modified => {
                                crate::proto::GitStatusSummary::Modified as i32
                            }
                            steer_workspace::GitStatusSummary::TypeChange => {
                                crate::proto::GitStatusSummary::TypeChange as i32
                            }
                            steer_workspace::GitStatusSummary::Renamed => {
                                crate::proto::GitStatusSummary::Renamed as i32
                            }
                            steer_workspace::GitStatusSummary::Copied => {
                                crate::proto::GitStatusSummary::Copied as i32
                            }
                            steer_workspace::GitStatusSummary::IntentToAdd => {
                                crate::proto::GitStatusSummary::IntentToAdd as i32
                            }
                            steer_workspace::GitStatusSummary::Conflict => {
                                crate::proto::GitStatusSummary::Conflict as i32
                            }
                        },
                        path: entry.path,
                    })
                    .collect();

                let recent_commits = status
                    .recent_commits
                    .into_iter()
                    .map(|commit| crate::proto::GitCommitSummary {
                        id: commit.id,
                        summary: commit.summary,
                    })
                    .collect();

                crate::proto::vcs_info::Status::GitStatus(crate::proto::GitStatus {
                    head,
                    entries,
                    recent_commits,
                    error: status.error,
                })
            }
            VcsStatus::Jj(status) => {
                let changes = status
                    .changes
                    .into_iter()
                    .map(|change| crate::proto::JjChange {
                        change_type: match change.change_type {
                            steer_workspace::JjChangeType::Added => {
                                crate::proto::JjChangeType::Added as i32
                            }
                            steer_workspace::JjChangeType::Removed => {
                                crate::proto::JjChangeType::Removed as i32
                            }
                            steer_workspace::JjChangeType::Modified => {
                                crate::proto::JjChangeType::Modified as i32
                            }
                        },
                        path: change.path,
                    })
                    .collect();

                let working_copy =
                    status
                        .working_copy
                        .map(|commit| crate::proto::JjCommitSummary {
                            change_id: commit.change_id,
                            commit_id: commit.commit_id,
                            description: commit.description,
                        });

                let parents = status
                    .parents
                    .into_iter()
                    .map(|commit| crate::proto::JjCommitSummary {
                        change_id: commit.change_id,
                        commit_id: commit.commit_id,
                        description: commit.description,
                    })
                    .collect();

                crate::proto::vcs_info::Status::JjStatus(crate::proto::JjStatus {
                    changes,
                    working_copy,
                    parents,
                    error: status.error,
                })
            }
        };

        crate::proto::VcsInfo {
            kind: kind as i32,
            root: info.root.to_string_lossy().to_string(),
            status: Some(status),
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
                Status::permission_denied(format!("Tool execution denied by user: {tool_name}"))
            }
            ToolError::DeniedByPolicy(tool_name) => {
                Status::permission_denied(format!("Tool execution denied by policy: {tool_name}"))
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

        let env_info = EnvironmentInfo::collect_for_path(Path::new(&working_directory))
            .map_err(|e| Status::internal(format!("Failed to collect environment info: {e}")))?;

        let response = crate::proto::GetEnvironmentInfoResponse {
            working_directory: env_info.working_directory.to_string_lossy().to_string(),
            vcs: env_info
                .vcs
                .map(RemoteWorkspaceService::convert_vcs_to_proto),
            platform: env_info.platform,
            date: env_info.date,
            directory_structure: env_info.directory_structure,
            readme_content: env_info.readme_content,
            memory_file_content: env_info.memory_file_content,
            memory_file_name: env_info.memory_file_name,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::tool_error_detail::Kind;

    #[test]
    fn test_tool_error_detail_denied_by_policy() {
        let detail =
            RemoteWorkspaceService::tool_error_to_detail(&ToolError::DeniedByPolicy("bash".into()));

        assert_eq!(detail.kind, Kind::DeniedByPolicy as i32);
        assert_eq!(detail.tool_name, "bash");
        assert!(detail.message.contains("denied by policy"));
    }
}
