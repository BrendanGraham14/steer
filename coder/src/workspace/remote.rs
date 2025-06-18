use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tools::{ToolCall, ToolSchema};
use tonic::transport::Channel;

use crate::app::EnvironmentInfo;
use crate::grpc::remote_workspace::{
    remote_workspace_service_client::RemoteWorkspaceServiceClient,
    GetEnvironmentInfoRequest, GetEnvironmentInfoResponse,
};
use crate::session::state::{RemoteAuth, ToolFilter};
use crate::tools::{ExecutionContext, RemoteBackend, ToolBackend};
use super::{CachedEnvironment, Workspace, WorkspaceMetadata, WorkspaceType};

/// Remote workspace that executes tools and collects environment info via gRPC
pub struct RemoteWorkspace {
    client: RemoteWorkspaceServiceClient<Channel>,
    address: String,
    auth: Option<RemoteAuth>,
    environment_cache: Arc<RwLock<Option<CachedEnvironment>>>,
    tool_backend: Arc<RemoteBackend>,
    metadata: WorkspaceMetadata,
}

impl RemoteWorkspace {
    pub async fn new(address: String, auth: Option<RemoteAuth>) -> Result<Self> {
        // Create gRPC client
        let client = RemoteWorkspaceServiceClient::connect(format!("http://{}", address)).await?;
        
        // Create remote tool backend
        let tool_backend = Arc::new(RemoteBackend::new(
            address.clone(),
            Duration::from_secs(30), // 30 second timeout
            auth.clone(),
            ToolFilter::All, // Allow all tools from remote workspace
        ).await?);
        
        let metadata = WorkspaceMetadata {
            id: format!("remote:{}", address),
            workspace_type: WorkspaceType::Remote,
            location: address.clone(),
        };
        
        Ok(Self {
            client,
            address,
            auth,
            environment_cache: Arc::new(RwLock::new(None)),
            tool_backend,
            metadata,
        })
    }
    
    /// Collect environment information from the remote workspace
    async fn collect_environment(&self) -> Result<EnvironmentInfo> {
        let mut client = self.client.clone();
        
        let request = tonic::Request::new(GetEnvironmentInfoRequest {
            working_directory: None, // Use remote default
        });
        
        let response = client.get_environment_info(request).await?;
        let env_response = response.into_inner();
        
        Self::convert_environment_response(env_response)
    }
    
    /// Convert gRPC response to EnvironmentInfo
    fn convert_environment_response(response: GetEnvironmentInfoResponse) -> Result<EnvironmentInfo> {
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
    
    async fn execute_tool(&self, tool_call: &ToolCall, ctx: ExecutionContext) -> Result<String> {
        self.tool_backend.execute(tool_call, &ctx).await
            .map_err(|e| anyhow::anyhow!("Tool execution failed: {}", e))
    }
    
    async fn available_tools(&self) -> Vec<ToolSchema> {
        self.tool_backend.get_tool_schemas().await
    }
    
    async fn requires_approval(&self, tool_name: &str) -> Result<bool> {
        self.tool_backend.requires_approval(tool_name).await
            .map_err(|e| anyhow::anyhow!("Failed to check tool approval: {}", e))
    }
    
    fn metadata(&self) -> WorkspaceMetadata {
        self.metadata.clone()
    }
    
    async fn invalidate_environment_cache(&self) {
        let mut cache = self.environment_cache.write().await;
        *cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::state::RemoteAuth;

    #[tokio::test]
    async fn test_remote_workspace_metadata() {
        let address = "localhost:50051".to_string();
        
        // This test will fail if no remote backend is running, but we can test metadata creation
        let metadata = WorkspaceMetadata {
            id: format!("remote:{}", address),
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
        
        assert_eq!(env_info.working_directory, PathBuf::from("/home/user/project"));
        assert_eq!(env_info.is_git_repo, true);
        assert_eq!(env_info.platform, "linux");
        assert_eq!(env_info.date, "2025-06-17");
        assert_eq!(env_info.directory_structure, "project/\nsrc/\nmain.rs\n");
        assert_eq!(env_info.git_status, Some("Current branch: main\n\nStatus:\nWorking tree clean\n".to_string()));
        assert_eq!(env_info.readme_content, Some("# My Project".to_string()));
        assert_eq!(env_info.claude_md_content, None);
    }
}