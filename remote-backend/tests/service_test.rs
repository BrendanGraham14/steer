use remote_backend::remote_backend_service::RemoteBackendService;
use remote_backend::proto::{
    ExecuteToolRequest, HealthStatus,
    remote_backend_service_server::RemoteBackendService as RemoteBackendServiceTrait,
};
use serde_json::json;
use tonic::Request;

/// Test service creation
#[tokio::test]
async fn test_service_creation() {
    let service = RemoteBackendService::new();
    assert!(service.is_ok());

    let service = service.unwrap();
    let tools = service.get_supported_tools();

    // Should have standard workspace tools
    assert!(tools.contains(&"bash".to_string()));
    assert!(tools.contains(&"read_file".to_string()));
    assert!(tools.contains(&"edit_file".to_string()));
}

/// Test health check endpoint
#[tokio::test]
async fn test_health_check() {
    let service = RemoteBackendService::new().unwrap();
    let request = Request::new(());

    let response = service.health(request).await;
    assert!(response.is_ok());

    let health = response.unwrap().into_inner();
    assert_eq!(health.status(), HealthStatus::Serving);
    assert!(!health.message.is_empty());
}

/// Test get_tool_schemas endpoint
#[tokio::test]
async fn test_get_tool_schemas() {
    let service = RemoteBackendService::new().unwrap();
    let request = Request::new(());

    let response = service.get_tool_schemas(request).await;
    assert!(response.is_ok());

    let schemas = response.unwrap().into_inner();
    assert!(!schemas.tools.is_empty());

    // Verify bash tool is present
    let bash_schema = schemas.tools.iter().find(|t| t.name == "bash");
    assert!(bash_schema.is_some());

    let bash = bash_schema.unwrap();
    assert!(!bash.description.is_empty());
    assert!(!bash.input_schema_json.is_empty());
    assert!(bash.requires_approval); // bash should require approval
}

/// Test tool execution with valid tool
#[tokio::test]
async fn test_execute_tool_ls() {
    let service = RemoteBackendService::new().unwrap();

    let request = Request::new(ExecuteToolRequest {
        tool_call_id: "test-123".to_string(),
        tool_name: "ls".to_string(),
        parameters_json: json!({
            "path": "/tmp"
        }).to_string(),
        context_json: "{}".to_string(), // Empty context for now
        timeout_ms: Some(5000),
    });

    let response = service.execute_tool(request).await;
    assert!(response.is_ok());

    let result = response.unwrap().into_inner();
    // Either succeeds or fails gracefully
    if result.success {
        assert!(!result.result.is_empty());
        assert!(result.error.is_empty());
    } else {
        assert!(!result.error.is_empty());
    }
}

/// Test tool execution with unknown tool
#[tokio::test]
async fn test_execute_unknown_tool() {
    let service = RemoteBackendService::new().unwrap();

    let request = Request::new(ExecuteToolRequest {
        tool_call_id: "test-456".to_string(),
        tool_name: "unknown_tool".to_string(),
        parameters_json: "{}".to_string(),
        context_json: "{}".to_string(),
        timeout_ms: Some(5000),
    });

    let response = service.execute_tool(request).await;
    assert!(response.is_err());

    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::NotFound);
    assert!(status.message().contains("Unknown tool"));
}

/// Test tool execution with invalid parameters
#[tokio::test]
async fn test_execute_tool_invalid_params() {
    let service = RemoteBackendService::new().unwrap();

    let request = Request::new(ExecuteToolRequest {
        tool_call_id: "test-789".to_string(),
        tool_name: "ls".to_string(),
        parameters_json: "invalid json".to_string(), // Invalid JSON
        context_json: "{}".to_string(),
        timeout_ms: Some(5000),
    });

    let response = service.execute_tool(request).await;
    assert!(response.is_err());

    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
}

/// Test tool cancellation
#[tokio::test]
async fn test_tool_cancellation() {
    let service = RemoteBackendService::new().unwrap();

    // Execute a long-running command
    let request = Request::new(ExecuteToolRequest {
        tool_call_id: "test-cancel".to_string(),
        tool_name: "bash".to_string(),
        parameters_json: json!({
            "command": "sleep 10"
        }).to_string(),
        context_json: "{}".to_string(),
        timeout_ms: Some(1000), // Short timeout to trigger cancellation
    });

    let handle = tokio::spawn(async move {
        service.execute_tool(request).await
    });

    // Give it a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Cancel the task
    handle.abort();

    // The task should be cancelled
    assert!(handle.await.is_err());
}

/// Test get_agent_info endpoint
#[tokio::test]
async fn test_get_agent_info() {
    let service = RemoteBackendService::new().unwrap();
    let request = Request::new(());

    let response = service.get_agent_info(request).await;
    assert!(response.is_ok());

    let info = response.unwrap().into_inner();
    assert!(!info.version.is_empty());
    assert!(!info.supported_tools.is_empty());
    assert!(info.capabilities.is_some());

    let capabilities = info.capabilities.unwrap();
    assert!(capabilities.supports_cancellation);
}

/// Test with_tools constructor
#[test]
fn test_with_tools_constructor() {
    use tools::tools::read_only_workspace_tools;

    let service = RemoteBackendService::with_tools(read_only_workspace_tools());
    let tools = service.get_supported_tools();

    // Should only have read-only tools
    assert!(tools.contains(&"read_file".to_string()));
    assert!(tools.contains(&"ls".to_string()));
    assert!(!tools.contains(&"bash".to_string())); // bash is not read-only
    assert!(!tools.contains(&"edit_file".to_string())); // edit is not read-only
}