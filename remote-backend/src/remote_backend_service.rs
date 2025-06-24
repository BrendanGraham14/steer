use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tonic::{Request, Response, Status};

use tools::tools::workspace_tools;
use tools::{ExecutionContext, Tool, ToolError};

use crate::proto::{
    AgentInfo, ExecuteToolRequest, ExecuteToolResponse, HealthResponse, HealthStatus, ToolSchema as GrpcToolSchema,
    ToolSchemasResponse,
    remote_workspace_service_server::RemoteWorkspaceService as RemoteWorkspaceServiceServer,
};

/// Agent service implementation that executes tools locally
///
/// This service receives tool execution requests via gRPC and executes them
/// using the standard tool executor. It's designed to run on remote machines,
/// VMs, or containers to provide remote tool execution capabilities.
pub struct RemoteWorkspaceService {
    tools: Arc<HashMap<String, Box<dyn Tool>>>,
    version: String,
}

impl RemoteWorkspaceService {
    /// Create a new RemoteWorkspaceService with the standard tool set
    pub fn new() -> Result<Self, ToolError> {
        Self::with_tools(workspace_tools())
    }

    /// Create a new RemoteWorkspaceService with a custom set of tools
    pub fn with_tools(tools_list: Vec<Box<dyn Tool>>) -> Result<Self, ToolError> {
        let mut tools: HashMap<String, Box<dyn Tool>> = HashMap::new();

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
    fn get_git_status(&self) -> Result<String, std::io::Error> {
        use std::process::Command;

        let mut result = String::new();

        // Get current branch
        if let Ok(output) = Command::new("git")
            .args(["branch", "--show-current"])
            .output()
        {
            if output.status.success() {
                let branch_output = String::from_utf8_lossy(&output.stdout);
                let branch = branch_output.trim().to_string();
                result.push_str(&format!("Current branch: {}\n\n", branch));
            }
        }

        // Get status
        if let Ok(output) = Command::new("git").args(["status", "--short"]).output() {
            if output.status.success() {
                let status_output = String::from_utf8_lossy(&output.stdout);
                let status = status_output.trim().to_string();
                result.push_str("Status:\n");
                if status.is_empty() {
                    result.push_str("Working tree clean\n");
                } else {
                    result.push_str(&status);
                    result.push('\n');
                }
            }
        }

        // Get recent commits
        if let Ok(output) = Command::new("git")
            .args(["log", "--oneline", "-n", "5"])
            .output()
        {
            if output.status.success() {
                let log_output = String::from_utf8_lossy(&output.stdout);
                let log = log_output.trim().to_string();
                result.push_str("\nRecent commits:\n");
                result.push_str(&log);
            }
        }

        Ok(result)
    }
}

#[tonic::async_trait]
impl RemoteWorkspaceServiceServer for RemoteWorkspaceService {
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

        let result = tool.execute(parameters, &context).await;

        let end_time = std::time::Instant::now();
        let duration = end_time - start_time;

        // Convert result to response
        let response = match result {
            Ok(output) => ExecuteToolResponse {
                success: true,
                result: output,
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
            },
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
                        result: String::new(),
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
        use std::process::Command;

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
        let is_git_repo = std::path::Path::new(".git").exists() || {
            Command::new("git")
                .args(["rev-parse", "--is-inside-work-tree"])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        };

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
            self.get_git_status().ok()
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
}
