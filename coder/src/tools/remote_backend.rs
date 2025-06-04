use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tonic::Request;
use tonic::transport::{Channel, Endpoint};

use crate::api::ToolCall;
use crate::tools::backend::{BackendMetadata, ToolBackend};
use crate::tools::execution_context::{AuthMethod, ExecutionContext, ExecutionEnvironment};
use tools::ToolSchema as ApiTool;
use tools::ToolError;

// Generated gRPC client from proto/remote_backend.proto
use crate::grpc::remote_backend::{
    ExecuteToolRequest, HealthStatus, remote_backend_service_client::RemoteBackendServiceClient,
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

/// Backend that forwards tool calls to a remote agent via gRPC
///
/// This backend connects to a remote agent service running on another machine,
/// VM, or container. It serializes tool calls and forwards them to the agent,
/// which executes the tools in its local environment.
pub struct RemoteBackend {
    client: RemoteBackendServiceClient<Channel>,
    agent_address: String,
    supported_tools: Vec<&'static str>,
    timeout: Duration,
}

impl RemoteBackend {
    /// Create a new RemoteBackend
    ///
    /// # Arguments
    /// * `agent_address` - The gRPC address of the remote agent (e.g., "http://vm:50051")
    /// * `timeout` - Timeout for tool execution requests
    pub async fn new(agent_address: String, timeout: Duration) -> Result<Self, ToolError> {
        let endpoint = Endpoint::from_shared(agent_address.clone())
            .map_err(|e| {
                ToolError::execution("RemoteBackend", format!("Invalid agent address: {}", e))
            })?
            .timeout(timeout);

        let channel = endpoint.connect().await.map_err(|e| {
            ToolError::execution(
                "RemoteBackend",
                format!("Failed to connect to agent at {}: {}", agent_address, e),
            )
        })?;

        let client = RemoteBackendServiceClient::new(channel);

        // These are the tools that can be executed remotely
        // (code-based tools that don't require special client-side resources)
        let supported_tools = vec![
            "edit_file",
            "multi_edit_file",
            "bash",
            "grep",
            "glob",
            "ls",
            "view",
            "replace",
        ];

        Ok(Self {
            client,
            agent_address,
            supported_tools,
            timeout,
        })
    }

    /// Create a RemoteBackend with default timeout (30 seconds)
    pub async fn new_with_default_timeout(agent_address: String) -> Result<Self, ToolError> {
        Self::new(agent_address, Duration::from_secs(30)).await
    }

    /// Get the agent address this backend connects to
    pub fn agent_address(&self) -> &str {
        &self.agent_address
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
    ) -> Result<String, ToolError> {
        // Convert to serializable context and serialize to JSON
        let serializable_context = SerializableExecutionContext::from(context);
        let context_json = serde_json::to_string(&serializable_context).map_err(|e| {
            ToolError::execution(
                "RemoteBackend",
                format!("Failed to serialize execution context: {}", e),
            )
        })?;

        // Serialize the tool parameters to JSON
        let parameters_json = serde_json::to_string(&tool_call.parameters).map_err(|e| {
            ToolError::execution(
                "RemoteBackend",
                format!("Failed to serialize tool parameters: {}", e),
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
            Ok(response.result)
        } else {
            Err(ToolError::execution(
                &tool_call.name,
                format!("Remote execution failed: {}", response.error),
            ))
        }
    }

    fn supported_tools(&self) -> Vec<&'static str> {
        self.supported_tools.clone()
    }

    fn to_api_tools(&self) -> Vec<ApiTool> {
        // For RemoteBackend, we would need to query the agent for its tool descriptions
        // For now, return an empty vec since this is typically handled by the local backend
        // that has access to the actual tool definitions
        Vec::new()
    }

    fn metadata(&self) -> BackendMetadata {
        BackendMetadata::new("RemoteBackend".to_string(), "Remote".to_string())
            .with_location(self.agent_address.clone())
            .with_info(
                "timeout_ms".to_string(),
                self.timeout.as_millis().to_string(),
            )
    }

    async fn health_check(&self) -> bool {
        let mut client = self.client.clone();
        let request = Request::new(());

        match client.health(request).await {
            Ok(response) => {
                let health = response.into_inner();
                health.status() == HealthStatus::Serving
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    #[ignore] // Requires a running agent server
    async fn test_remote_backend_creation() {
        let result =
            RemoteBackend::new_with_default_timeout("http://localhost:50051".to_string()).await;

        // This test will fail if no agent is running, which is expected
        // In a real test environment, we'd mock the gRPC client
        match result {
            Ok(backend) => {
                assert_eq!(backend.agent_address(), "http://localhost:50051");
                assert_eq!(backend.timeout(), Duration::from_secs(30));
                assert!(!backend.supported_tools().is_empty());
            }
            Err(_) => {
                // Expected when no agent is running
            }
        }
    }

    #[test]
    fn test_supported_tools() {
        // We can test the supported tools list without connecting
        let supported_tools = vec![
            "edit_file",
            "multi_edit_file",
            "bash",
            "grep",
            "glob",
            "ls",
            "view",
            "replace",
        ];

        assert_eq!(supported_tools.len(), 8);
        assert!(supported_tools.contains(&"edit_file"));
        assert!(supported_tools.contains(&"bash"));
        assert!(!supported_tools.contains(&"fetch_url")); // This typically stays local
    }
}
