use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tonic::transport::Channel;

use steer_proto::remote_workspace::v1::{
    ApplyEditsRequest as ProtoApplyEditsRequest, AstGrepRequest as ProtoAstGrepRequest,
    EditOperation as ProtoEditOperation, GetEnvironmentInfoRequest, GetEnvironmentInfoResponse,
    GlobRequest as ProtoGlobRequest, GrepRequest as ProtoGrepRequest,
    ListDirectoryRequest as ProtoListDirectoryRequest, ListFilesRequest,
    ReadFileRequest as ProtoReadFileRequest, WriteFileRequest as ProtoWriteFileRequest,
    remote_workspace_service_client::RemoteWorkspaceServiceClient,
};
use steer_tools::result::{
    EditResult, FileContentResult, FileEntry, FileListResult, GlobResult, SearchMatch, SearchResult,
};
use steer_workspace::{
    ApplyEditsRequest, AstGrepRequest, EnvironmentInfo, GitCommitSummary, GitHead, GitStatus,
    GitStatusEntry, GitStatusSummary, GlobRequest, GrepRequest, JjChange, JjChangeType,
    JjCommitSummary, JjStatus, ListDirectoryRequest, ReadFileRequest, RemoteAuth, Result, VcsInfo,
    VcsKind, VcsStatus, Workspace, WorkspaceError, WorkspaceMetadata, WorkspaceOpContext,
    WorkspaceType, WriteFileRequest,
};

const GRPC_MAX_MESSAGE_SIZE_BYTES: usize = 32 * 1024 * 1024;

fn convert_search_result(proto_result: steer_proto::common::v1::SearchResult) -> SearchResult {
    let matches = proto_result
        .matches
        .into_iter()
        .map(|m| SearchMatch {
            file_path: m.file_path,
            line_number: m.line_number as usize,
            line_content: m.line_content,
            column_range: m
                .column_range
                .map(|cr| (cr.start as usize, cr.end as usize)),
        })
        .collect();

    SearchResult {
        matches,
        total_files_searched: proto_result.total_files_searched as usize,
        search_completed: proto_result.search_completed,
    }
}

fn convert_file_list_result(
    proto_result: steer_proto::common::v1::FileListResult,
) -> FileListResult {
    let entries = proto_result
        .entries
        .into_iter()
        .map(|e| FileEntry {
            path: e.path,
            is_directory: e.is_directory,
            size: e.size,
            permissions: e.permissions,
        })
        .collect();

    FileListResult {
        entries,
        base_path: proto_result.base_path,
    }
}

fn convert_file_content_result(
    proto_result: steer_proto::common::v1::FileContentResult,
) -> FileContentResult {
    FileContentResult {
        content: proto_result.content,
        file_path: proto_result.file_path,
        line_count: proto_result.line_count as usize,
        truncated: proto_result.truncated,
    }
}

fn convert_edit_result(proto_result: steer_proto::common::v1::EditResult) -> EditResult {
    EditResult {
        file_path: proto_result.file_path,
        changes_made: proto_result.changes_made as usize,
        file_created: proto_result.file_created,
        old_content: proto_result.old_content,
        new_content: proto_result.new_content,
    }
}

fn convert_glob_result(proto_result: steer_proto::common::v1::GlobResult) -> GlobResult {
    GlobResult {
        matches: proto_result.matches,
        pattern: proto_result.pattern,
    }
}

/// Cached environment information with TTL
#[derive(Debug, Clone)]
struct CachedEnvironment {
    pub info: EnvironmentInfo,
    pub cached_at: std::time::Instant,
    pub ttl: Duration,
}

impl CachedEnvironment {
    pub fn new(info: EnvironmentInfo, ttl: Duration) -> Self {
        Self {
            info,
            cached_at: std::time::Instant::now(),
            ttl,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.cached_at.elapsed() > self.ttl
    }
}

/// Remote workspace that executes tools and collects environment info via gRPC
pub struct RemoteWorkspace {
    client: RemoteWorkspaceServiceClient<Channel>,
    environment_cache: Arc<RwLock<Option<CachedEnvironment>>>,
    metadata: WorkspaceMetadata,
    #[allow(dead_code)]
    auth: Option<RemoteAuth>,
}

impl RemoteWorkspace {
    pub async fn new(address: String, auth: Option<RemoteAuth>) -> Result<Self> {
        // Create gRPC client
        let client = RemoteWorkspaceServiceClient::connect(format!("http://{address}"))
            .await
            .map_err(|e| WorkspaceError::Transport(format!("Failed to connect: {e}")))?
            .max_decoding_message_size(GRPC_MAX_MESSAGE_SIZE_BYTES)
            .max_encoding_message_size(GRPC_MAX_MESSAGE_SIZE_BYTES);

        let metadata = WorkspaceMetadata {
            id: format!("remote:{address}"),
            workspace_type: WorkspaceType::Remote,
            location: address.clone(),
        };

        Ok(Self {
            client,
            environment_cache: Arc::new(RwLock::new(None)),
            metadata,
            auth,
        })
    }

    /// Collect environment information from the remote workspace
    async fn collect_environment(&self) -> Result<EnvironmentInfo> {
        let mut client = self.client.clone();

        let request = tonic::Request::new(GetEnvironmentInfoRequest {
            working_directory: None, // Use remote default
        });

        let response = client
            .get_environment_info(request)
            .await
            .map_err(|e| WorkspaceError::Status(format!("Failed to get environment info: {e}")))?;
        let env_response = response.into_inner();

        Self::convert_environment_response(env_response)
    }

    /// Convert gRPC response to EnvironmentInfo
    fn convert_environment_response(
        response: GetEnvironmentInfoResponse,
    ) -> Result<EnvironmentInfo> {
        use std::path::PathBuf;
        use steer_proto::remote_workspace::v1::{
            GitHeadKind as ProtoGitHeadKind, GitStatusSummary as ProtoGitStatusSummary,
            JjChangeType as ProtoJjChangeType, VcsKind as ProtoVcsKind, vcs_info,
        };

        let vcs = response.vcs.and_then(|vcs| {
            let kind = match ProtoVcsKind::try_from(vcs.kind).ok()? {
                ProtoVcsKind::Git => VcsKind::Git,
                ProtoVcsKind::Jj => VcsKind::Jj,
                ProtoVcsKind::Unspecified => return None,
            };

            let status = match vcs.status {
                Some(vcs_info::Status::GitStatus(status)) => {
                    let head = status.head.and_then(|head| {
                        let kind = ProtoGitHeadKind::try_from(head.kind).ok()?;
                        match kind {
                            ProtoGitHeadKind::Branch => {
                                Some(GitHead::Branch(head.branch.unwrap_or_default()))
                            }
                            ProtoGitHeadKind::Detached => Some(GitHead::Detached),
                            ProtoGitHeadKind::Unborn => Some(GitHead::Unborn),
                            ProtoGitHeadKind::Unspecified => None,
                        }
                    });

                    let entries = status
                        .entries
                        .into_iter()
                        .filter_map(|entry| {
                            let summary = match ProtoGitStatusSummary::try_from(entry.summary)
                                .ok()?
                            {
                                ProtoGitStatusSummary::Added => GitStatusSummary::Added,
                                ProtoGitStatusSummary::Removed => GitStatusSummary::Removed,
                                ProtoGitStatusSummary::Modified => GitStatusSummary::Modified,
                                ProtoGitStatusSummary::TypeChange => GitStatusSummary::TypeChange,
                                ProtoGitStatusSummary::Renamed => GitStatusSummary::Renamed,
                                ProtoGitStatusSummary::Copied => GitStatusSummary::Copied,
                                ProtoGitStatusSummary::IntentToAdd => GitStatusSummary::IntentToAdd,
                                ProtoGitStatusSummary::Conflict => GitStatusSummary::Conflict,
                                ProtoGitStatusSummary::Unspecified => return None,
                            };
                            Some(GitStatusEntry {
                                summary,
                                path: entry.path,
                            })
                        })
                        .collect();

                    let recent_commits = status
                        .recent_commits
                        .into_iter()
                        .map(|commit| GitCommitSummary {
                            id: commit.id,
                            summary: commit.summary,
                        })
                        .collect();

                    VcsStatus::Git(GitStatus {
                        head,
                        entries,
                        recent_commits,
                        error: status.error,
                    })
                }
                Some(vcs_info::Status::JjStatus(status)) => {
                    let changes = status
                        .changes
                        .into_iter()
                        .filter_map(|change| {
                            let change_type =
                                match ProtoJjChangeType::try_from(change.change_type).ok()? {
                                    ProtoJjChangeType::Added => JjChangeType::Added,
                                    ProtoJjChangeType::Removed => JjChangeType::Removed,
                                    ProtoJjChangeType::Modified => JjChangeType::Modified,
                                    ProtoJjChangeType::Unspecified => return None,
                                };
                            Some(JjChange {
                                change_type,
                                path: change.path,
                            })
                        })
                        .collect();

                    let working_copy = status.working_copy.map(|commit| JjCommitSummary {
                        change_id: commit.change_id,
                        commit_id: commit.commit_id,
                        description: commit.description,
                    });

                    let parents = status
                        .parents
                        .into_iter()
                        .map(|commit| JjCommitSummary {
                            change_id: commit.change_id,
                            commit_id: commit.commit_id,
                            description: commit.description,
                        })
                        .collect();

                    VcsStatus::Jj(JjStatus {
                        changes,
                        working_copy,
                        parents,
                        error: status.error,
                    })
                }
                None => match kind {
                    VcsKind::Git => {
                        VcsStatus::Git(GitStatus::unavailable("missing git status".to_string()))
                    }
                    VcsKind::Jj => {
                        VcsStatus::Jj(JjStatus::unavailable("missing jj status".to_string()))
                    }
                },
            };

            Some(VcsInfo {
                kind,
                root: PathBuf::from(vcs.root),
                status,
            })
        });

        Ok(EnvironmentInfo {
            working_directory: PathBuf::from(response.working_directory),
            vcs,
            platform: response.platform,
            date: response.date,
            directory_structure: response.directory_structure,
            readme_content: response.readme_content,
            memory_file_name: response.memory_file_name,
            memory_file_content: response.memory_file_content,
        })
    }
}

impl std::fmt::Debug for RemoteWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteWorkspace")
            .field("metadata", &self.metadata)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Workspace for RemoteWorkspace {
    async fn environment(&self) -> Result<EnvironmentInfo> {
        let mut cache = self.environment_cache.write().await;

        // Check if we have valid cached data
        if let Some(cached) = cache.as_ref()
            && !cached.is_expired()
        {
            return Ok(cached.info.clone());
        }

        // Collect fresh environment info from remote
        let env_info = self.collect_environment().await?;

        // Cache it with 10 minute TTL (longer than local since remote calls are expensive)
        *cache = Some(CachedEnvironment::new(
            env_info.clone(),
            Duration::from_secs(600), // 10 minutes
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
    ) -> Result<Vec<String>> {
        let mut client = self.client.clone();

        let request = tonic::Request::new(ListFilesRequest {
            query: query.unwrap_or("").to_string(),
            max_results: max_results.unwrap_or(0) as u32,
        });

        let mut stream = client
            .list_files(request)
            .await
            .map_err(|e| WorkspaceError::Status(format!("Failed to list files: {e}")))?
            .into_inner();
        let mut all_files = Vec::new();

        // Collect all files from the stream
        while let Some(response) = stream
            .message()
            .await
            .map_err(|e| WorkspaceError::Status(format!("Stream error: {e}")))?
        {
            all_files.extend(response.paths);
        }

        Ok(all_files)
    }

    fn working_directory(&self) -> &std::path::Path {
        // For remote workspaces, we return a placeholder path
        // The actual working directory is on the remote machine
        std::path::Path::new("/remote")
    }

    async fn read_file(
        &self,
        request: ReadFileRequest,
        _ctx: &WorkspaceOpContext,
    ) -> Result<FileContentResult> {
        let mut client = self.client.clone();
        let request = tonic::Request::new(ProtoReadFileRequest {
            file_path: request.file_path,
            offset: request.offset,
            limit: request.limit,
        });
        let response = client
            .read_file(request)
            .await
            .map_err(|e| WorkspaceError::Status(format!("Failed to read file: {e}")))?
            .into_inner();
        Ok(convert_file_content_result(response))
    }

    async fn list_directory(
        &self,
        request: ListDirectoryRequest,
        _ctx: &WorkspaceOpContext,
    ) -> Result<FileListResult> {
        let mut client = self.client.clone();
        let request = tonic::Request::new(ProtoListDirectoryRequest {
            path: request.path,
            ignore: request.ignore.unwrap_or_default(),
        });
        let response = client
            .list_directory(request)
            .await
            .map_err(|e| WorkspaceError::Status(format!("Failed to list directory: {e}")))?
            .into_inner();
        Ok(convert_file_list_result(response))
    }

    async fn glob(&self, request: GlobRequest, _ctx: &WorkspaceOpContext) -> Result<GlobResult> {
        let mut client = self.client.clone();
        let request = tonic::Request::new(ProtoGlobRequest {
            pattern: request.pattern,
            path: request.path,
        });
        let response = client
            .glob(request)
            .await
            .map_err(|e| WorkspaceError::Status(format!("Failed to glob: {e}")))?
            .into_inner();
        Ok(convert_glob_result(response))
    }

    async fn grep(&self, request: GrepRequest, _ctx: &WorkspaceOpContext) -> Result<SearchResult> {
        let mut client = self.client.clone();
        let request = tonic::Request::new(ProtoGrepRequest {
            pattern: request.pattern,
            include: request.include,
            path: request.path,
        });
        let response = client
            .grep(request)
            .await
            .map_err(|e| WorkspaceError::Status(format!("Failed to grep: {e}")))?
            .into_inner();
        Ok(convert_search_result(response))
    }

    async fn astgrep(
        &self,
        request: AstGrepRequest,
        _ctx: &WorkspaceOpContext,
    ) -> Result<SearchResult> {
        let mut client = self.client.clone();
        let request = tonic::Request::new(ProtoAstGrepRequest {
            pattern: request.pattern,
            lang: request.lang,
            include: request.include,
            exclude: request.exclude,
            path: request.path,
        });
        let response = client
            .ast_grep(request)
            .await
            .map_err(|e| WorkspaceError::Status(format!("Failed to astgrep: {e}")))?
            .into_inner();
        Ok(convert_search_result(response))
    }

    async fn apply_edits(
        &self,
        request: ApplyEditsRequest,
        _ctx: &WorkspaceOpContext,
    ) -> Result<EditResult> {
        let mut client = self.client.clone();
        let edits = request
            .edits
            .into_iter()
            .map(|edit| ProtoEditOperation {
                old_string: edit.old_string,
                new_string: edit.new_string,
            })
            .collect();
        let request = tonic::Request::new(ProtoApplyEditsRequest {
            file_path: request.file_path,
            edits,
        });
        let response = client
            .apply_edits(request)
            .await
            .map_err(|e| WorkspaceError::Status(format!("Failed to apply edits: {e}")))?
            .into_inner();
        Ok(convert_edit_result(response))
    }

    async fn write_file(
        &self,
        request: WriteFileRequest,
        _ctx: &WorkspaceOpContext,
    ) -> Result<EditResult> {
        let mut client = self.client.clone();
        let request = tonic::Request::new(ProtoWriteFileRequest {
            file_path: request.file_path,
            content: request.content,
        });
        let response = client
            .write_file(request)
            .await
            .map_err(|e| WorkspaceError::Status(format!("Failed to write file: {e}")))?
            .into_inner();
        Ok(convert_edit_result(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steer_workspace::LlmStatus;

    #[tokio::test]
    async fn test_remote_workspace_metadata() {
        let address = "localhost:50051".to_string();

        // This test will fail if no remote backend is running, but we can test metadata creation
        let metadata = WorkspaceMetadata {
            id: format!("remote:{address}"),
            workspace_type: WorkspaceType::Remote,
            location: address.clone(),
        };

        assert!(matches!(metadata.workspace_type, WorkspaceType::Remote));
        assert_eq!(metadata.location, address);
    }

    #[test]
    fn test_convert_environment_response() {
        use std::path::PathBuf;

        let response = GetEnvironmentInfoResponse {
            working_directory: "/home/user/project".to_string(),
            vcs: Some(steer_proto::remote_workspace::v1::VcsInfo {
                kind: steer_proto::remote_workspace::v1::VcsKind::Git as i32,
                root: "/home/user/project".to_string(),
                status: Some(
                    steer_proto::remote_workspace::v1::vcs_info::Status::GitStatus(
                        steer_proto::remote_workspace::v1::GitStatus {
                            head: Some(steer_proto::remote_workspace::v1::GitHead {
                                kind: steer_proto::remote_workspace::v1::GitHeadKind::Branch as i32,
                                branch: Some("main".to_string()),
                            }),
                            entries: Vec::new(),
                            recent_commits: Vec::new(),
                            error: None,
                        },
                    ),
                ),
            }),
            platform: "linux".to_string(),
            date: "2025-06-17".to_string(),
            directory_structure: "project/\nsrc/\nmain.rs\n".to_string(),
            readme_content: Some("# My Project".to_string()),
            memory_file_content: None,
            memory_file_name: None,
        };

        // Test the static conversion function directly
        let env_info = RemoteWorkspace::convert_environment_response(response).unwrap();

        assert_eq!(
            env_info.working_directory,
            PathBuf::from("/home/user/project")
        );
        assert!(matches!(
            env_info.vcs,
            Some(VcsInfo {
                kind: VcsKind::Git,
                ..
            })
        ));
        assert_eq!(env_info.platform, "linux");
        assert_eq!(env_info.date, "2025-06-17");
        assert_eq!(env_info.directory_structure, "project/\nsrc/\nmain.rs\n");
        assert_eq!(
            env_info
                .vcs
                .as_ref()
                .map(|vcs| vcs.status.as_llm_string()),
            Some(
                "Current branch: main\n\nStatus:\nWorking tree clean\n\nRecent commits:\n<no commits>\n"
                    .to_string()
            )
        );
        assert_eq!(env_info.readme_content, Some("# My Project".to_string()));
        assert_eq!(env_info.memory_file_content, None);
        assert_eq!(env_info.memory_file_name, None);
    }
}
