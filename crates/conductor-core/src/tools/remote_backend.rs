use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tonic::service::Interceptor;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status};

use crate::api::ToolCall;
use crate::session::state::{RemoteAuth, ToolFilter};
use crate::tools::backend::{BackendMetadata, ToolBackend};
use crate::tools::execution_context::{ExecutionContext, ExecutionEnvironment};
use conductor_tools::result::{
    BashResult, EditResult, FileContentResult, FileEntry, FileListResult, GlobResult, SearchMatch,
    SearchResult, TodoItem, TodoListResult, TodoWriteResult,
};
use conductor_tools::{ToolError, ToolSchema, result::ToolResult};

// Generated gRPC client from proto/remote_workspace.proto
use conductor_proto::remote_workspace::v1::{
    ExecuteToolRequest, GetToolApprovalRequirementsRequest, GetToolSchemasRequest, HealthRequest,
    HealthStatus, execute_tool_response::Result as ProtoResult,
    remote_workspace_service_client::RemoteWorkspaceServiceClient,
};

/// Serializable version of ExecutionContext for remote transmission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableExecutionContext {
    pub session_id: String,
    pub operation_id: String,
    pub tool_call_id: String,
    pub timeout_ms: u64,
    pub environment: ExecutionEnvironment,
}

impl From<&ExecutionContext> for SerializableExecutionContext {
    fn from(context: &ExecutionContext) -> Self {
        Self {
            session_id: context.session_id.clone(),
            operation_id: context.operation_id.clone(),
            tool_call_id: context.tool_call_id.clone(),
            timeout_ms: context.timeout.as_millis() as u64,
            environment: context.environment.clone(),
        }
    }
}

/// Authentication interceptor for gRPC requests
#[derive(Clone)]
struct AuthInterceptor {
    auth: RemoteAuth,
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        match &self.auth {
            RemoteAuth::Bearer { token } => {
                request.metadata_mut().insert(
                    "authorization",
                    format!("Bearer {token}")
                        .parse()
                        .map_err(|_| Status::internal("Failed to parse authorization header"))?,
                );
            }
            RemoteAuth::ApiKey { key } => {
                request.metadata_mut().insert(
                    "x-api-key",
                    key.parse()
                        .map_err(|_| Status::internal("Failed to parse API key header"))?,
                );
            }
        }
        Ok(request)
    }
}

/// Backend that forwards tool calls to a remote agent via gRPC
///
/// This backend connects to a remote agent service running on another machine,
/// VM, or container. It serializes tool calls and forwards them to the agent,
/// which executes the tools in its local environment.
pub struct RemoteBackend {
    client: RemoteWorkspaceServiceClient<
        tonic::service::interceptor::InterceptedService<Channel, AuthInterceptor>,
    >,
    address: String,
    supported_tools: Vec<String>,
    timeout: Duration,
}

impl RemoteBackend {
    /// Create a new RemoteBackend with tool filtering and authentication
    ///
    /// # Arguments
    /// * `address` - The gRPC address of the remote backend (e.g., "http://vm:50051")
    /// * `timeout` - Timeout for tool execution requests
    /// * `tool_filter` - Tool filtering configuration (All, Include, or Exclude)
    /// * `auth` - Optional authentication configuration
    pub async fn new(
        agent_address: String,
        timeout: Duration,
        auth: Option<RemoteAuth>,
        tool_filter: ToolFilter,
    ) -> Result<Self, ToolError> {
        let endpoint = Endpoint::from_shared(agent_address.clone())
            .map_err(|e| {
                ToolError::execution("RemoteBackend", format!("Invalid agent address: {e}"))
            })?
            .timeout(timeout);

        let channel = endpoint.connect().await.map_err(|e| {
            ToolError::execution(
                "RemoteBackend",
                format!("Failed to connect to agent at {agent_address}: {e}"),
            )
        })?;

        // Create client with or without authentication interceptor
        let client = match auth {
            Some(auth_config) => {
                let interceptor = AuthInterceptor { auth: auth_config };
                RemoteWorkspaceServiceClient::with_interceptor(channel, interceptor)
            }
            None => {
                // Create a no-op interceptor for consistent client type
                let interceptor = AuthInterceptor {
                    auth: RemoteAuth::ApiKey { key: String::new() },
                };
                RemoteWorkspaceServiceClient::with_interceptor(channel, interceptor)
            }
        };

        // Fetch available tools from the remote agent
        let mut client_clone = client.clone();
        let all_remote_tools = match client_clone
            .get_tool_schemas(Request::new(GetToolSchemasRequest {}))
            .await
        {
            Ok(response) => response
                .into_inner()
                .tools
                .into_iter()
                .map(|s| s.name)
                .collect::<Vec<_>>(),
            Err(status) => {
                return Err(ToolError::execution(
                    "RemoteBackend",
                    format!("Failed to get tool schemas from agent: {status}"),
                ));
            }
        };

        // Filter tools based on the tool filter configuration
        let supported_tools = match tool_filter {
            ToolFilter::All => all_remote_tools,
            ToolFilter::Include(included) => {
                let all_remote_tools_set: std::collections::HashSet<String> =
                    all_remote_tools.into_iter().collect();
                included
                    .into_iter()
                    .filter(|t| all_remote_tools_set.contains(t))
                    .collect()
            }
            ToolFilter::Exclude(excluded) => {
                let excluded_set: std::collections::HashSet<String> =
                    excluded.into_iter().collect();
                all_remote_tools
                    .into_iter()
                    .filter(|t| !excluded_set.contains(t))
                    .collect()
            }
        };

        Ok(Self {
            client,
            address: agent_address,
            supported_tools,
            timeout,
        })
    }

    /// Get the agent address this backend connects to
    pub fn agent_address(&self) -> &str {
        &self.address
    }

    /// Get the configured timeout
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[async_trait]
impl ToolBackend for RemoteBackend {
    async fn execute(
        &self,
        tool_call: &ToolCall,
        context: &ExecutionContext,
    ) -> Result<ToolResult, ToolError> {
        // Convert to serializable context and serialize to JSON
        let serializable_context = SerializableExecutionContext::from(context);
        let context_json = serde_json::to_string(&serializable_context).map_err(|e| {
            ToolError::execution(
                "RemoteBackend",
                format!("Failed to serialize execution context: {e}"),
            )
        })?;

        // Serialize the tool parameters to JSON
        let parameters_json = serde_json::to_string(&tool_call.parameters).map_err(|e| {
            ToolError::execution(
                "RemoteBackend",
                format!("Failed to serialize tool parameters: {e}"),
            )
        })?;

        // Create the gRPC request
        let request = Request::new(ExecuteToolRequest {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            parameters_json,
            context_json,
            timeout_ms: Some(self.timeout.as_millis() as u64),
        });

        // Execute the remote call
        let mut client = self.client.clone();
        let response = client.execute_tool(request).await.map_err(|status| {
            ToolError::execution(
                "RemoteBackend",
                format!("gRPC call failed: {} ({})", status.message(), status.code()),
            )
        })?;

        let response = response.into_inner();

        // Check if the execution was successful
        if response.success {
            // Handle the oneof result
            match response.result {
                Some(ProtoResult::StringResult(s)) => {
                    // Legacy string result - treat as External
                    Ok(ToolResult::External(
                        conductor_tools::result::ExternalResult {
                            tool_name: tool_call.name.clone(),
                            payload: s,
                        },
                    ))
                }
                Some(ProtoResult::TypedResult(typed)) => {
                    // Generic typed result - treat as External
                    let payload = typed
                        .summary
                        .unwrap_or_else(|| format!("Result of type: {}", typed.type_name));
                    Ok(ToolResult::External(
                        conductor_tools::result::ExternalResult {
                            tool_name: tool_call.name.clone(),
                            payload,
                        },
                    ))
                }
                Some(ProtoResult::SearchResult(proto_result)) => {
                    // Convert proto SearchResult to our SearchResult
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
                None => {
                    // No result provided
                    Err(ToolError::execution(
                        &tool_call.name,
                        "Remote execution succeeded but returned no result",
                    ))
                }
            }
        } else {
            Err(ToolError::execution(
                &tool_call.name,
                format!("Remote execution failed: {}", response.error),
            ))
        }
    }

    async fn supported_tools(&self) -> Vec<String> {
        self.supported_tools.clone()
    }

    async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        // Query the remote agent for tool schemas
        let mut client = self.client.clone();
        let request = Request::new(GetToolSchemasRequest {});

        match client.get_tool_schemas(request).await {
            Ok(response) => {
                let schemas = response.into_inner();
                schemas
                    .tools
                    .into_iter()
                    .map(|schema| {
                        // Parse the JSON input schema
                        let input_schema = serde_json::from_str(&schema.input_schema_json)
                            .unwrap_or_else(|_| conductor_tools::InputSchema {
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

    fn metadata(&self) -> BackendMetadata {
        BackendMetadata::new("RemoteBackend".to_string(), "Remote".to_string())
            .with_location(self.address.clone())
            .with_info(
                "timeout_ms".to_string(),
                self.timeout.as_millis().to_string(),
            )
    }

    async fn health_check(&self) -> bool {
        let mut client = self.client.clone();
        let request = Request::new(HealthRequest {});

        match client.health(request).await {
            Ok(response) => {
                let health = response.into_inner();
                health.status() == HealthStatus::Serving
            }
            Err(_) => false,
        }
    }

    async fn requires_approval(&self, tool_name: &str) -> Result<bool, ToolError> {
        // Check if this tool is supported by this backend
        if self.supported_tools.iter().any(|s| s == tool_name) {
            // Make an async RPC call to get approval requirements
            let mut client = self.client.clone();
            let request = Request::new(GetToolApprovalRequirementsRequest {
                tool_names: vec![tool_name.to_string()],
            });

            match client.get_tool_approval_requirements(request).await {
                Ok(response) => {
                    let resp = response.into_inner();
                    resp.approval_requirements
                        .get(tool_name)
                        .copied()
                        .ok_or_else(|| ToolError::UnknownTool(tool_name.to_string()))
                }
                Err(status) => Err(ToolError::execution(
                    "RemoteBackend",
                    format!(
                        "Failed to get approval requirements: {} ({})",
                        status.message(),
                        status.code()
                    ),
                )),
            }
        } else {
            Err(ToolError::UnknownTool(tool_name.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_proto::remote_workspace::v1::{
        ExecuteToolResponse, GetAgentInfoRequest, GetAgentInfoResponse, GetEnvironmentInfoRequest,
        GetEnvironmentInfoResponse, GetToolApprovalRequirementsResponse, GetToolSchemasResponse,
        HealthResponse, ListFilesRequest, ListFilesResponse, ToolSchema as GrpcToolSchema,
        remote_workspace_service_server::{RemoteWorkspaceService, RemoteWorkspaceServiceServer},
    };
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tonic::transport::Server;

    struct MockRemoteBackend {
        tool_names: Vec<String>,
    }

    #[async_trait]
    impl RemoteWorkspaceService for MockRemoteBackend {
        async fn execute_tool(
            &self,
            _request: Request<ExecuteToolRequest>,
        ) -> Result<tonic::Response<ExecuteToolResponse>, Status> {
            Ok(tonic::Response::new(ExecuteToolResponse {
                success: true,
                result: Some(
                    conductor_proto::remote_workspace::v1::execute_tool_response::Result::StringResult(
                        "mocked".to_string(),
                    ),
                ),
                ..Default::default()
            }))
        }

        async fn get_agent_info(
            &self,
            _request: Request<GetAgentInfoRequest>,
        ) -> Result<tonic::Response<GetAgentInfoResponse>, Status> {
            unimplemented!()
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<tonic::Response<HealthResponse>, Status> {
            unimplemented!()
        }

        async fn get_tool_schemas(
            &self,
            _request: Request<GetToolSchemasRequest>,
        ) -> Result<tonic::Response<GetToolSchemasResponse>, Status> {
            let tools = self
                .tool_names
                .iter()
                .map(|name| GrpcToolSchema {
                    name: name.clone(),
                    description: format!("Description for {name}"),
                    input_schema_json: "{}".to_string(),
                })
                .collect();
            Ok(tonic::Response::new(GetToolSchemasResponse { tools }))
        }

        async fn get_tool_approval_requirements(
            &self,
            _request: Request<GetToolApprovalRequirementsRequest>,
        ) -> Result<tonic::Response<GetToolApprovalRequirementsResponse>, Status> {
            unimplemented!()
        }

        async fn get_environment_info(
            &self,
            _request: Request<GetEnvironmentInfoRequest>,
        ) -> Result<tonic::Response<GetEnvironmentInfoResponse>, Status> {
            unimplemented!()
        }

        type ListFilesStream = tonic::codec::Streaming<ListFilesResponse>;

        async fn list_files(
            &self,
            _request: Request<ListFilesRequest>,
        ) -> Result<tonic::Response<Self::ListFilesStream>, Status> {
            unimplemented!()
        }
    }

    async fn setup_mock_server(tool_names: Vec<String>) -> SocketAddr {
        let service = MockRemoteBackend { tool_names };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let _server_task = tokio::spawn(async move {
            Server::builder()
                .add_service(RemoteWorkspaceServiceServer::new(service))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        addr
    }

    #[tokio::test]
    async fn test_remote_backend_creation_all_tools() {
        let addr = setup_mock_server(vec![
            "tool1".to_string(),
            "tool2".to_string(),
            "bash".to_string(),
        ])
        .await;
        let backend = RemoteBackend::new(
            format!("http://{addr}"),
            Duration::from_secs(5),
            None,
            ToolFilter::All,
        )
        .await
        .unwrap();

        assert_eq!(backend.supported_tools().await.len(), 3);
        assert!(
            backend
                .supported_tools()
                .await
                .contains(&"tool1".to_string())
        );
    }

    #[tokio::test]
    async fn test_remote_backend_creation_filtered_tools() {
        let addr = setup_mock_server(vec![
            "tool1".to_string(),
            "tool2".to_string(),
            "bash".to_string(),
        ])
        .await;
        let backend = RemoteBackend::new(
            format!("http://{addr}"),
            Duration::from_secs(5),
            None,
            ToolFilter::Include(vec!["tool1".to_string(), "bash".to_string()]),
        )
        .await
        .unwrap();

        let supported = backend.supported_tools().await;
        assert_eq!(supported.len(), 2);
        assert!(supported.contains(&"tool1".to_string()));
        assert!(supported.contains(&"bash".to_string()));
        assert!(!supported.contains(&"tool2".to_string()));
    }

    #[tokio::test]
    async fn test_remote_backend_with_auth() {
        let addr = setup_mock_server(vec!["tool1".to_string()]).await;
        let auth = RemoteAuth::Bearer {
            token: "test-token".to_string(),
        };
        let backend = RemoteBackend::new(
            format!("http://{addr}"),
            Duration::from_secs(5),
            Some(auth),
            ToolFilter::All,
        )
        .await
        .unwrap();

        assert_eq!(backend.agent_address(), format!("http://{addr}"));
        assert_eq!(backend.supported_tools().await.len(), 1);
    }
}
