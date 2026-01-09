use steer_remote_workspace::proto::{
    ExecuteToolRequest, GetAgentInfoRequest, GetToolSchemasRequest, HealthRequest, HealthStatus,
    ListDirectoryRequest, ReadFileRequest, WriteFileRequest,
    remote_workspace_service_server::RemoteWorkspaceService as RemoteWorkspaceServiceTrait,
};
use steer_remote_workspace::remote_workspace_service::RemoteWorkspaceService;
use tempfile::tempdir;
use tonic::Request;

#[tokio::test]
async fn test_service_creation() {
    let service = RemoteWorkspaceService::new(std::env::temp_dir()).await;
    assert!(service.is_ok());

    let service = service.unwrap();
    let tools = service.get_supported_tools();
    assert!(tools.is_empty());
}

#[tokio::test]
async fn test_health_check() {
    let service = RemoteWorkspaceService::new(std::env::temp_dir())
        .await
        .unwrap();
    let request = Request::new(HealthRequest {});

    let response = service.health(request).await;
    assert!(response.is_ok());

    let health = response.unwrap().into_inner();
    assert_eq!(health.status(), HealthStatus::Serving);
    assert!(!health.message.is_empty());
}

#[tokio::test]
async fn test_get_tool_schemas_empty() {
    let service = RemoteWorkspaceService::new(std::env::temp_dir())
        .await
        .unwrap();
    let request = Request::new(GetToolSchemasRequest {});

    let response = service.get_tool_schemas(request).await;
    assert!(response.is_ok());

    let schemas = response.unwrap().into_inner();
    assert!(schemas.tools.is_empty());
}

#[tokio::test]
async fn test_execute_tool_unimplemented() {
    let service = RemoteWorkspaceService::new(std::env::temp_dir())
        .await
        .unwrap();

    let request = Request::new(ExecuteToolRequest {
        tool_call_id: "test-123".to_string(),
        tool_name: "ls".to_string(),
        parameters_json: "{}".to_string(),
        context_json: "{}".to_string(),
        timeout_ms: Some(5000),
    });

    let response = service.execute_tool(request).await;
    assert!(response.is_err());
    let status = response.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unimplemented);
}

#[tokio::test]
async fn test_write_and_read_file() {
    let temp_dir = tempdir().unwrap();
    let service = RemoteWorkspaceService::new(temp_dir.path().to_path_buf())
        .await
        .unwrap();

    let file_path = temp_dir.path().join("hello.txt");
    let write_req = Request::new(WriteFileRequest {
        file_path: file_path.to_string_lossy().to_string(),
        content: "hello world\n".to_string(),
    });

    let write_response = service.write_file(write_req).await;
    assert!(write_response.is_ok());

    let read_req = Request::new(ReadFileRequest {
        file_path: file_path.to_string_lossy().to_string(),
        offset: None,
        limit: None,
    });
    let read_response = service.read_file(read_req).await;
    assert!(read_response.is_ok());
    let content = read_response.unwrap().into_inner();
    assert!(content.content.contains("hello world"));
}

#[tokio::test]
async fn test_list_directory() {
    let temp_dir = tempdir().unwrap();
    let service = RemoteWorkspaceService::new(temp_dir.path().to_path_buf())
        .await
        .unwrap();

    std::fs::write(temp_dir.path().join("file.txt"), "contents").unwrap();

    let request = Request::new(ListDirectoryRequest {
        path: temp_dir.path().to_string_lossy().to_string(),
        ignore: Vec::new(),
    });

    let response = service.list_directory(request).await;
    assert!(response.is_ok());
    let list = response.unwrap().into_inner();
    assert!(list.entries.iter().any(|e| e.path == "file.txt"));
}

#[tokio::test]
async fn test_get_agent_info() {
    let service = RemoteWorkspaceService::new(std::env::temp_dir())
        .await
        .unwrap();
    let request = Request::new(GetAgentInfoRequest {});

    let response = service.get_agent_info(request).await;
    assert!(response.is_ok());

    let info = response.unwrap().into_inner();
    assert!(!info.version.is_empty());
    assert!(info.metadata.contains_key("working_directory"));
}
