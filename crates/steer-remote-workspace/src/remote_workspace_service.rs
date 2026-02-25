use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use steer_workspace::local::LocalWorkspace;
use steer_workspace::{VcsInfo, VcsKind, VcsStatus, Workspace, WorkspaceError, WorkspaceOpContext};

use crate::proto::{
    ApplyEditsRequest as GrpcApplyEditsRequest, AstGrepRequest as GrpcAstGrepRequest,
    ExecuteToolRequest, ExecuteToolResponse, GetAgentInfoRequest, GetAgentInfoResponse,
    GetToolApprovalRequirementsRequest, GetToolApprovalRequirementsResponse, GetToolSchemasRequest,
    GetToolSchemasResponse, GlobRequest as GrpcGlobRequest, GrepRequest as GrpcGrepRequest,
    HealthRequest, HealthResponse, HealthStatus, ListDirectoryRequest as GrpcListDirectoryRequest,
    ListFilesRequest, ListFilesResponse, ReadFileRequest as GrpcReadFileRequest,
    WriteFileRequest as GrpcWriteFileRequest,
    remote_workspace_service_server::RemoteWorkspaceService as RemoteWorkspaceServiceServer,
};
use steer_proto::common::v1::{
    ColumnRange as ProtoColumnRange, EditResult as ProtoEditResult,
    FileContentResult as ProtoFileContentResult, FileEntry as ProtoFileEntry,
    FileListResult as ProtoFileListResult, GlobResult as ProtoGlobResult,
    SearchMatch as ProtoSearchMatch, SearchResult as ProtoSearchResult,
};

/// Remote workspace service that exposes workspace operations over gRPC.
pub struct RemoteWorkspaceService {
    workspace: Arc<LocalWorkspace>,
    version: String,
}

impl RemoteWorkspaceService {
    /// Create a new RemoteWorkspaceService backed by a local workspace.
    pub async fn new(working_dir: PathBuf) -> Result<Self, WorkspaceError> {
        let workspace = LocalWorkspace::with_path(working_dir).await?;
        Ok(Self {
            workspace: Arc::new(workspace),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    /// Get the supported tool names for legacy compatibility.
    pub fn get_supported_tools(&self) -> Vec<String> {
        Vec::new()
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

    fn search_result_to_proto(search_result: &steer_workspace::SearchResult) -> ProtoSearchResult {
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

        ProtoSearchResult {
            matches: proto_matches,
            total_files_searched: search_result.total_files_searched as u64,
            search_completed: search_result.search_completed,
        }
    }

    fn file_list_result_to_proto(
        file_list: &steer_workspace::FileListResult,
    ) -> ProtoFileListResult {
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

        ProtoFileListResult {
            entries: proto_entries,
            base_path: file_list.base_path.clone(),
        }
    }

    fn file_content_result_to_proto(
        file_content: &steer_workspace::FileContentResult,
    ) -> ProtoFileContentResult {
        ProtoFileContentResult {
            content: file_content.content.clone(),
            file_path: file_content.file_path.clone(),
            line_count: file_content.line_count as u64,
            truncated: file_content.truncated,
        }
    }

    fn edit_result_to_proto(edit_result: &steer_workspace::EditResult) -> ProtoEditResult {
        ProtoEditResult {
            file_path: edit_result.file_path.clone(),
            changes_made: edit_result.changes_made as u64,
            file_created: edit_result.file_created,
            old_content: edit_result.old_content.clone(),
            new_content: edit_result.new_content.clone(),
        }
    }

    fn glob_result_to_proto(glob_result: &steer_workspace::GlobResult) -> ProtoGlobResult {
        ProtoGlobResult {
            matches: glob_result.matches.clone(),
            pattern: glob_result.pattern.clone(),
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
        Ok(Response::new(GetToolSchemasResponse { tools: Vec::new() }))
    }

    /// Execute a tool call on the agent (legacy API).
    async fn execute_tool(
        &self,
        _request: Request<ExecuteToolRequest>,
    ) -> Result<Response<ExecuteToolResponse>, Status> {
        Err(Status::unimplemented(
            "execute_tool is deprecated; use workspace ops instead",
        ))
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
                    self.workspace
                        .working_directory()
                        .to_string_lossy()
                        .to_string(),
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
            message: "Workspace service is healthy and ready to execute operations".to_string(),
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
        _request: Request<GetToolApprovalRequirementsRequest>,
    ) -> Result<Response<GetToolApprovalRequirementsResponse>, Status> {
        let approval_requirements = std::collections::HashMap::new();
        Ok(Response::new(GetToolApprovalRequirementsResponse {
            approval_requirements,
        }))
    }

    /// Get environment information for the remote workspace
    async fn get_environment_info(
        &self,
        request: Request<crate::proto::GetEnvironmentInfoRequest>,
    ) -> Result<Response<crate::proto::GetEnvironmentInfoResponse>, Status> {
        let _req = request.into_inner();
        let env_info =
            self.workspace.environment().await.map_err(|e| {
                Status::internal(format!("Failed to collect environment info: {e}"))
            })?;

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
        let workspace = self.workspace.clone();

        // Create the response stream
        let (tx, rx) = mpsc::channel(100);

        // Spawn task to stream the files
        tokio::spawn(async move {
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

            let files = match workspace.list_files(query, max_results).await {
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

    async fn read_file(
        &self,
        request: Request<GrpcReadFileRequest>,
    ) -> Result<Response<ProtoFileContentResult>, Status> {
        let req = request.into_inner();
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let _guard = cancellation_token.clone().drop_guard();
        let context = WorkspaceOpContext::new("read_file", cancellation_token);
        let params = steer_workspace::ReadFileRequest {
            file_path: req.file_path,
            offset: req.offset,
            limit: req.limit,
            raw: req.raw,
        };

        let result = self
            .workspace
            .read_file(params, &context)
            .await
            .map_err(|e| Status::internal(format!("ReadFile failed: {e}")))?;

        Ok(Response::new(Self::file_content_result_to_proto(&result)))
    }

    async fn list_directory(
        &self,
        request: Request<GrpcListDirectoryRequest>,
    ) -> Result<Response<ProtoFileListResult>, Status> {
        let req = request.into_inner();
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let _guard = cancellation_token.clone().drop_guard();
        let context = WorkspaceOpContext::new("ls", cancellation_token);
        let params = steer_workspace::ListDirectoryRequest {
            path: req.path,
            ignore: if req.ignore.is_empty() {
                None
            } else {
                Some(req.ignore)
            },
        };

        let result = self
            .workspace
            .list_directory(params, &context)
            .await
            .map_err(|e| Status::internal(format!("ListDirectory failed: {e}")))?;

        Ok(Response::new(Self::file_list_result_to_proto(&result)))
    }

    async fn glob(
        &self,
        request: Request<GrpcGlobRequest>,
    ) -> Result<Response<ProtoGlobResult>, Status> {
        let req = request.into_inner();
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let _guard = cancellation_token.clone().drop_guard();
        let context = WorkspaceOpContext::new("glob", cancellation_token);
        let params = steer_workspace::GlobRequest {
            pattern: req.pattern,
            path: req.path,
        };

        let result = self
            .workspace
            .glob(params, &context)
            .await
            .map_err(|e| Status::internal(format!("Glob failed: {e}")))?;

        Ok(Response::new(Self::glob_result_to_proto(&result)))
    }

    async fn grep(
        &self,
        request: Request<GrpcGrepRequest>,
    ) -> Result<Response<ProtoSearchResult>, Status> {
        let req = request.into_inner();
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let _guard = cancellation_token.clone().drop_guard();
        let context = WorkspaceOpContext::new("grep", cancellation_token);
        let params = steer_workspace::GrepRequest {
            pattern: req.pattern,
            include: req.include,
            path: req.path,
        };

        let result = self
            .workspace
            .grep(params, &context)
            .await
            .map_err(|e| Status::internal(format!("Grep failed: {e}")))?;

        Ok(Response::new(Self::search_result_to_proto(&result)))
    }

    async fn ast_grep(
        &self,
        request: Request<GrpcAstGrepRequest>,
    ) -> Result<Response<ProtoSearchResult>, Status> {
        let req = request.into_inner();
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let _guard = cancellation_token.clone().drop_guard();
        let context = WorkspaceOpContext::new("astgrep", cancellation_token);
        let params = steer_workspace::AstGrepRequest {
            pattern: req.pattern,
            lang: req.lang,
            include: req.include,
            exclude: req.exclude,
            path: req.path,
        };

        let result = self
            .workspace
            .astgrep(params, &context)
            .await
            .map_err(|e| Status::internal(format!("AstGrep failed: {e}")))?;

        Ok(Response::new(Self::search_result_to_proto(&result)))
    }

    async fn apply_edits(
        &self,
        request: Request<GrpcApplyEditsRequest>,
    ) -> Result<Response<ProtoEditResult>, Status> {
        let req = request.into_inner();
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let _guard = cancellation_token.clone().drop_guard();
        let context = WorkspaceOpContext::new("edit", cancellation_token);
        let edits = req
            .edits
            .into_iter()
            .map(|edit| steer_workspace::EditOperation {
                old_string: edit.old_string,
                new_string: edit.new_string,
            })
            .collect::<Vec<_>>();

        let params = steer_workspace::ApplyEditsRequest {
            file_path: req.file_path,
            edits,
        };

        let result = self
            .workspace
            .apply_edits(params, &context)
            .await
            .map_err(|e| Status::internal(format!("ApplyEdits failed: {e}")))?;

        Ok(Response::new(Self::edit_result_to_proto(&result)))
    }

    async fn write_file(
        &self,
        request: Request<GrpcWriteFileRequest>,
    ) -> Result<Response<ProtoEditResult>, Status> {
        let req = request.into_inner();
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let _guard = cancellation_token.clone().drop_guard();
        let context = WorkspaceOpContext::new("write_file", cancellation_token);
        let params = steer_workspace::WriteFileRequest {
            file_path: req.file_path,
            content: req.content,
        };

        let result = self
            .workspace
            .write_file(params, &context)
            .await
            .map_err(|e| Status::internal(format!("WriteFile failed: {e}")))?;

        Ok(Response::new(Self::edit_result_to_proto(&result)))
    }
}
