use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tonic::transport::Channel;

use steer_proto::remote_workspace::v1::{
    ExecuteToolRequest, ExecuteToolResponse, GetEnvironmentInfoRequest, GetEnvironmentInfoResponse,
    GetToolApprovalRequirementsRequest, GetToolSchemasRequest, ListFilesRequest,
    remote_workspace_service_client::RemoteWorkspaceServiceClient,
};
use steer_tools::{ToolCall, ToolSchema, result::ToolResult};
use steer_workspace::{
    EnvironmentInfo, RemoteAuth, Result, Workspace, WorkspaceError, WorkspaceMetadata,
    WorkspaceType,
};

/// Serializable version of ExecutionContext for remote transmission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableExecutionContext {
    pub tool_call_id: String,
    pub working_directory: std::path::PathBuf,
}

impl From<&steer_tools::ExecutionContext> for SerializableExecutionContext {
    fn from(context: &steer_tools::ExecutionContext) -> Self {
        Self {
            tool_call_id: context.tool_call_id.clone(),
            working_directory: context.working_directory.clone(),
        }
    }
}

/// Convert gRPC tool response to ToolResult
fn convert_tool_response(response: ExecuteToolResponse) -> Result<ToolResult> {
    use steer_proto::remote_workspace::v1::execute_tool_response::Result as ProtoResult;
    use steer_tools::result::{
        BashResult, EditResult, ExternalResult, FileContentResult, FileEntry, FileListResult,
        GlobResult, SearchMatch, SearchResult, TodoItem, TodoListResult, TodoWriteResult,
    };

    match response.result {
        Some(ProtoResult::StringResult(s)) => {
            // Legacy string result - treat as External
            Ok(ToolResult::External(ExternalResult {
                tool_name: "remote".to_string(),
                payload: s,
            }))
        }
        Some(ProtoResult::SearchResult(proto_result)) => {
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

            Ok(ToolResult::Search(SearchResult {
                matches,
                total_files_searched: proto_result.total_files_searched as usize,
                search_completed: proto_result.search_completed,
            }))
        }
        Some(ProtoResult::FileListResult(proto_result)) => {
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

            Ok(ToolResult::FileList(FileListResult {
                entries,
                base_path: proto_result.base_path,
            }))
        }
        Some(ProtoResult::FileContentResult(proto_result)) => {
            Ok(ToolResult::FileContent(FileContentResult {
                content: proto_result.content,
                file_path: proto_result.file_path,
                line_count: proto_result.line_count as usize,
                truncated: proto_result.truncated,
            }))
        }
        Some(ProtoResult::EditResult(proto_result)) => Ok(ToolResult::Edit(EditResult {
            file_path: proto_result.file_path,
            changes_made: proto_result.changes_made as usize,
            file_created: proto_result.file_created,
            old_content: proto_result.old_content,
            new_content: proto_result.new_content,
        })),
        Some(ProtoResult::BashResult(proto_result)) => Ok(ToolResult::Bash(BashResult {
            stdout: proto_result.stdout,
            stderr: proto_result.stderr,
            exit_code: proto_result.exit_code,
            command: proto_result.command,
        })),
        Some(ProtoResult::GlobResult(proto_result)) => Ok(ToolResult::Glob(GlobResult {
            matches: proto_result.matches,
            pattern: proto_result.pattern,
        })),
        Some(ProtoResult::TodoListResult(proto_result)) => {
            let todos = proto_result
                .todos
                .into_iter()
                .map(|t| TodoItem {
                    id: t.id,
                    content: t.content,
                    status: t.status,
                    priority: t.priority,
                })
                .collect();

            Ok(ToolResult::TodoRead(TodoListResult { todos }))
        }
        Some(ProtoResult::TodoWriteResult(proto_result)) => {
            let todos = proto_result
                .todos
                .into_iter()
                .map(|t| TodoItem {
                    id: t.id,
                    content: t.content,
                    status: t.status,
                    priority: t.priority,
                })
                .collect();

            Ok(ToolResult::TodoWrite(TodoWriteResult {
                todos,
                operation: proto_result.operation,
            }))
        }
        _ => Err(WorkspaceError::ToolExecution(
            "No result returned from remote execution".to_string(),
        )),
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
            .map_err(|e| WorkspaceError::Transport(format!("Failed to connect: {e}")))?;

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

        Ok(EnvironmentInfo {
            working_directory: PathBuf::from(response.working_directory),
            is_git_repo: response.is_git_repo,
            platform: response.platform,
            date: response.date,
            directory_structure: response.directory_structure,
            git_status: response.git_status,
            readme_content: response.readme_content,
            claude_md_content: response.claude_md_content,
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
        if let Some(cached) = cache.as_ref() {
            if !cached.is_expired() {
                return Ok(cached.info.clone());
            }
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

    async fn execute_tool(
        &self,
        tool_call: &ToolCall,
        context: steer_tools::ExecutionContext,
    ) -> Result<ToolResult> {
        let mut client = self.client.clone();

        // Serialize the execution context
        let context_json = serde_json::to_string(&SerializableExecutionContext::from(&context))
            .map_err(|e| {
                WorkspaceError::ToolExecution(format!("Failed to serialize context: {e}"))
            })?;

        // Serialize tool parameters
        let parameters_json = serde_json::to_string(&tool_call.parameters).map_err(|e| {
            WorkspaceError::ToolExecution(format!("Failed to serialize parameters: {e}"))
        })?;

        let request = tonic::Request::new(ExecuteToolRequest {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            parameters_json,
            context_json,
            timeout_ms: Some(30000), // 30 second default
        });

        let response = client
            .execute_tool(request)
            .await
            .map_err(|e| WorkspaceError::ToolExecution(format!("Failed to execute tool: {e}")))?
            .into_inner();

        if !response.success {
            return Err(WorkspaceError::ToolExecution(format!(
                "Tool execution failed: {}",
                response.error
            )));
        }

        // Convert the response to ToolResult
        convert_tool_response(response)
    }

    async fn available_tools(&self) -> Vec<ToolSchema> {
        let mut client = self.client.clone();

        let request = tonic::Request::new(GetToolSchemasRequest {});

        match client.get_tool_schemas(request).await {
            Ok(response) => {
                response
                    .into_inner()
                    .tools
                    .into_iter()
                    .map(|schema| {
                        // Parse the JSON input schema
                        let input_schema = serde_json::from_str(&schema.input_schema_json)
                            .unwrap_or_else(|_| steer_tools::InputSchema {
                                properties: serde_json::Map::new(),
                                required: Vec::new(),
                                schema_type: "object".to_string(),
                            });

                        ToolSchema {
                            name: schema.name,
                            description: schema.description,
                            input_schema,
                        }
                    })
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }

    async fn requires_approval(&self, tool_name: &str) -> Result<bool> {
        let mut client = self.client.clone();

        let request = tonic::Request::new(GetToolApprovalRequirementsRequest {
            tool_names: vec![tool_name.to_string()],
        });

        let response = client
            .get_tool_approval_requirements(request)
            .await
            .map_err(|e| {
                WorkspaceError::ToolExecution(format!("Failed to get approval requirements: {e}"))
            })?
            .into_inner();

        response
            .approval_requirements
            .get(tool_name)
            .copied()
            .ok_or_else(|| WorkspaceError::ToolExecution(format!("Unknown tool: {tool_name}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            is_git_repo: true,
            platform: "linux".to_string(),
            date: "2025-06-17".to_string(),
            directory_structure: "project/\nsrc/\nmain.rs\n".to_string(),
            git_status: Some("Current branch: main\n\nStatus:\nWorking tree clean\n".to_string()),
            readme_content: Some("# My Project".to_string()),
            claude_md_content: None,
        };

        // Test the static conversion function directly
        let env_info = RemoteWorkspace::convert_environment_response(response).unwrap();

        assert_eq!(
            env_info.working_directory,
            PathBuf::from("/home/user/project")
        );
        assert!(env_info.is_git_repo);
        assert_eq!(env_info.platform, "linux");
        assert_eq!(env_info.date, "2025-06-17");
        assert_eq!(env_info.directory_structure, "project/\nsrc/\nmain.rs\n");
        assert_eq!(
            env_info.git_status,
            Some("Current branch: main\n\nStatus:\nWorking tree clean\n".to_string())
        );
        assert_eq!(env_info.readme_content, Some("# My Project".to_string()));
        assert_eq!(env_info.claude_md_content, None);
    }
}
