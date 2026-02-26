use steer_remote_workspace::proto::{
    ApplyEditsRequest, EditMatchAll, EditMatchExactlyOne, EditMatchNth, EditOperation,
    ExecuteToolRequest, GetAgentInfoRequest, GetToolSchemasRequest, HealthRequest, HealthStatus,
    ListDirectoryRequest, ReadFileRequest, WriteFileRequest, edit_operation,
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
        raw: None,
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
async fn test_apply_edits_supports_match_mode_all() {
    let temp_dir = tempdir().unwrap();
    let service = RemoteWorkspaceService::new(temp_dir.path().to_path_buf())
        .await
        .unwrap();

    let file_path = temp_dir.path().join("multi.txt");
    std::fs::write(&file_path, "repeat\nrepeat\n").unwrap();

    let edit_request = Request::new(ApplyEditsRequest {
        file_path: file_path.to_string_lossy().to_string(),
        edits: vec![EditOperation {
            old_string: "repeat".to_string(),
            new_string: "done".to_string(),
            match_selection: Some(edit_operation::MatchSelection::All(EditMatchAll {})),
        }],
    });

    let edit_response = service.apply_edits(edit_request).await;
    assert!(edit_response.is_ok());
    let edit_result = edit_response.unwrap().into_inner();
    assert_eq!(edit_result.changes_made, 2);

    let read_request = Request::new(ReadFileRequest {
        file_path: file_path.to_string_lossy().to_string(),
        offset: None,
        limit: None,
        raw: Some(true),
    });
    let read_response = service.read_file(read_request).await;
    assert!(read_response.is_ok());
    let content = read_response.unwrap().into_inner();
    assert_eq!(content.content, "done\ndone\n");
}

#[tokio::test]
async fn test_apply_edits_supports_match_mode_nth_and_match_index() {
    let temp_dir = tempdir().unwrap();
    let service = RemoteWorkspaceService::new(temp_dir.path().to_path_buf())
        .await
        .unwrap();

    let file_path = temp_dir.path().join("multi.txt");
    std::fs::write(&file_path, "repeat\nrepeat\nrepeat\n").unwrap();

    let edit_request = Request::new(ApplyEditsRequest {
        file_path: file_path.to_string_lossy().to_string(),
        edits: vec![EditOperation {
            old_string: "repeat".to_string(),
            new_string: "done".to_string(),
            match_selection: Some(edit_operation::MatchSelection::Nth(EditMatchNth {
                match_index: 2,
            })),
        }],
    });

    let edit_response = service.apply_edits(edit_request).await;
    assert!(edit_response.is_ok());
    let edit_result = edit_response.unwrap().into_inner();
    assert_eq!(edit_result.changes_made, 1);

    let read_request = Request::new(ReadFileRequest {
        file_path: file_path.to_string_lossy().to_string(),
        offset: None,
        limit: None,
        raw: Some(true),
    });
    let read_response = service.read_file(read_request).await;
    assert!(read_response.is_ok());
    let content = read_response.unwrap().into_inner();
    assert_eq!(content.content, "repeat\ndone\nrepeat\n");
}

#[tokio::test]
async fn test_apply_edits_supports_match_selection_exactly_one() {
    let temp_dir = tempdir().unwrap();
    let service = RemoteWorkspaceService::new(temp_dir.path().to_path_buf())
        .await
        .unwrap();

    let file_path = temp_dir.path().join("multi.txt");
    std::fs::write(&file_path, "repeat\n").unwrap();

    let edit_request = Request::new(ApplyEditsRequest {
        file_path: file_path.to_string_lossy().to_string(),
        edits: vec![EditOperation {
            old_string: "repeat".to_string(),
            new_string: "done".to_string(),
            match_selection: Some(edit_operation::MatchSelection::ExactlyOne(
                EditMatchExactlyOne {},
            )),
        }],
    });

    let response = service
        .apply_edits(edit_request)
        .await
        .expect("exactly_one selector should be valid");
    assert_eq!(response.into_inner().changes_made, 1);
}

#[tokio::test]
async fn test_apply_edits_nth_requires_match_index_over_grpc() {
    let temp_dir = tempdir().unwrap();
    let service = RemoteWorkspaceService::new(temp_dir.path().to_path_buf())
        .await
        .unwrap();

    let file_path = temp_dir.path().join("multi.txt");
    std::fs::write(&file_path, "repeat\nrepeat\n").unwrap();

    let edit_request = Request::new(ApplyEditsRequest {
        file_path: file_path.to_string_lossy().to_string(),
        edits: vec![EditOperation {
            old_string: "repeat".to_string(),
            new_string: "done".to_string(),
            match_selection: Some(edit_operation::MatchSelection::Nth(EditMatchNth {
                match_index: 0,
            })),
        }],
    });

    let err = service
        .apply_edits(edit_request)
        .await
        .expect_err("nth index 0 should fail");
    assert_eq!(err.code(), tonic::Code::Internal);
    assert!(err.message().contains("must be 1 or greater"));
}

#[tokio::test]
async fn test_apply_edits_nth_rejects_out_of_range_match_index_over_grpc() {
    let temp_dir = tempdir().unwrap();
    let service = RemoteWorkspaceService::new(temp_dir.path().to_path_buf())
        .await
        .unwrap();

    let file_path = temp_dir.path().join("multi.txt");
    std::fs::write(&file_path, "repeat\nrepeat\n").unwrap();

    let edit_request = Request::new(ApplyEditsRequest {
        file_path: file_path.to_string_lossy().to_string(),
        edits: vec![EditOperation {
            old_string: "repeat".to_string(),
            new_string: "done".to_string(),
            match_selection: Some(edit_operation::MatchSelection::Nth(EditMatchNth {
                match_index: 3,
            })),
        }],
    });

    let err = service
        .apply_edits(edit_request)
        .await
        .expect_err("nth out of range should fail");
    assert_eq!(err.code(), tonic::Code::Internal);
    assert!(err.message().contains("out of range"));
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
