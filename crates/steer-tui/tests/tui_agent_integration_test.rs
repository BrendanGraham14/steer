use std::path::PathBuf;
use std::sync::Arc;

use steer_core::session::SessionManagerConfig;
use steer_grpc::{ServiceHost, ServiceHostConfig};
use steer_proto::agent::v1::{
    CreateSessionRequest, ListFilesRequest, SendMessageRequest, SubscribeRequest, WorkspaceConfig,
    agent_service_client::AgentServiceClient, stream_session_request,
};
use steer_tui::error::Result;
use tempfile::TempDir;
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tracing::{debug, info};

/// Create a test workspace with some files
async fn setup_test_workspace() -> Result<(TempDir, PathBuf)> {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let workspace_path = temp_dir.path().to_path_buf();

    // Create some test files
    tokio::fs::create_dir_all(workspace_path.join("src")).await?;
    tokio::fs::create_dir_all(workspace_path.join("src/utils")).await?;
    tokio::fs::create_dir_all(workspace_path.join("tests")).await?;

    // Write test files
    tokio::fs::write(
        workspace_path.join("Cargo.toml"),
        r#"[package]
name = "test-project"
version = "0.1.0"
edition = "2024"
"#,
    )
    .await
    .expect("Failed to write Cargo.toml");

    tokio::fs::write(
        workspace_path.join("README.md"),
        "# Test Project\n\nThis is a test project for fuzzy finder testing.\n",
    )
    .await
    .expect("Failed to write README.md");

    tokio::fs::write(
        workspace_path.join("src/main.rs"),
        r#"fn main() {
    println!("Hello, world!");
}
"#,
    )
    .await
    .expect("Failed to write src/main.rs");

    tokio::fs::write(
        workspace_path.join("src/lib.rs"),
        r#"//! Test library
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#,
    )
    .await?;

    tokio::fs::write(
        workspace_path.join("src/utils/helper.rs"),
        r#"pub fn helper_function() -> &'static str {
    "I'm a helper!"
}
"#,
    )
    .await?;

    tokio::fs::write(
        workspace_path.join("tests/integration_test.rs"),
        r#"#[test]
fn test_something() {
    assert_eq!(2 + 2, 4);
}
"#,
    )
    .await?;

    // Create a .gitignore file
    tokio::fs::write(workspace_path.join(".gitignore"), "target/\n*.swp\n").await?;

    Ok((temp_dir, workspace_path))
}

#[tokio::test]
async fn test_tui_agent_service_file_listing() {
    // Initialize logging
    let _ = tracing_subscriber::fmt::try_init();

    // Setup test workspace
    let (_temp_dir, workspace_path) = setup_test_workspace()
        .await
        .expect("Failed to setup test workspace");
    info!("Created test workspace at: {:?}", workspace_path);

    // Create ServiceHost configuration with explicit port and test auth storage
    let db_path = workspace_path.join("test_sessions.db");
    let bind_addr = "127.0.0.1:50051".parse().unwrap(); // Use fixed port for testing
    let config = ServiceHostConfig {
        db_path,
        session_manager_config: SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: steer_core::config::model::builtin::claude_3_5_sonnet_20241022(),
            auto_persist: true,
        },
        bind_addr,
        auth_storage: Arc::new(steer_core::test_utils::InMemoryAuthStorage::new()),
    };

    // Start the service host
    let mut service_host = ServiceHost::new(config).await.unwrap();
    service_host.start().await.unwrap();

    // Wait a bit for server to be ready
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    info!("Started gRPC server at: {}", bind_addr);

    // Create gRPC client
    let channel = Channel::from_shared(format!("http://{bind_addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut grpc_client = AgentServiceClient::new(channel.clone());

    // Test 1: Create a session with local workspace
    let create_req = CreateSessionRequest {
        workspace_config: Some(WorkspaceConfig {
            config: Some(steer_proto::agent::v1::workspace_config::Config::Local(
                steer_proto::agent::v1::LocalWorkspaceConfig {
                    path: workspace_path.to_string_lossy().to_string(),
                },
            )),
        }),
        metadata: [("test".to_string(), "true".to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    };

    let create_resp = grpc_client.create_session(create_req).await.unwrap();
    let session_response = create_resp.into_inner();
    let session_id = session_response.session.unwrap().id.clone();
    info!("Created session: {}", session_id);

    // Test 2: List files in the workspace
    let list_files_req = ListFilesRequest {
        session_id: session_id.clone(),
        query: String::new(), // Empty query to get all files
        max_results: 0,       // 0 means no limit
    };

    let mut file_stream = grpc_client
        .list_files(list_files_req)
        .await
        .unwrap()
        .into_inner();
    let mut all_files = Vec::new();

    while let Some(response) = file_stream.message().await.unwrap() {
        debug!("Received {} files in chunk", response.paths.len());
        all_files.extend(response.paths);
    }

    info!("Received {} total files from server", all_files.len());

    // Verify we got the expected files
    assert!(
        all_files.len() >= 6,
        "Should have at least 6 files, got {}",
        all_files.len()
    );
    assert!(
        all_files.iter().any(|f| f.ends_with("main.rs")),
        "Should have main.rs in: {all_files:?}"
    );
    assert!(
        all_files.iter().any(|f| f.ends_with("lib.rs")),
        "Should have lib.rs"
    );
    assert!(
        all_files.iter().any(|f| f.ends_with("Cargo.toml")),
        "Should have Cargo.toml"
    );
    assert!(
        all_files.iter().any(|f| f.ends_with("README.md")),
        "Should have README.md"
    );
    assert!(
        all_files.iter().any(|f| f.ends_with("helper.rs")),
        "Should have helper.rs"
    );

    // Test 3: List files with query filter
    let list_files_req = ListFilesRequest {
        session_id: session_id.clone(),
        query: "main".to_string(),
        max_results: 10,
    };

    let mut file_stream = grpc_client
        .list_files(list_files_req)
        .await
        .expect("Failed to list files with query")
        .into_inner();
    let mut filtered_files = Vec::new();

    while let Some(response) = file_stream
        .message()
        .await
        .expect("Failed to receive filtered file list message")
    {
        filtered_files.extend(response.paths);
    }

    info!("Received {} files matching 'main'", filtered_files.len());
    assert!(
        !filtered_files.is_empty(),
        "Should have files matching 'main'"
    );
    assert!(
        filtered_files.iter().all(|f| f.contains("main")),
        "All results should contain 'main': {filtered_files:?}"
    );

    // Test 4: Verify file paths are relative to workspace
    for file in &all_files {
        assert!(
            !file.starts_with('/'),
            "File paths should be relative, got: {file}"
        );
        assert!(
            !file.contains(&workspace_path.to_string_lossy().to_string()),
            "File paths should not contain absolute workspace path: {file}"
        );
    }

    // Cleanup
    service_host
        .shutdown()
        .await
        .expect("Failed to shutdown service host");
}

#[tokio::test]
async fn test_tui_fuzzy_finder_with_grpc_events() {
    let _ = tracing_subscriber::fmt::try_init();

    // Setup
    let (_temp_dir, workspace_path) = setup_test_workspace().await.unwrap();

    // Start service
    let db_path = workspace_path.join("test_sessions.db");
    let bind_addr = "127.0.0.1:50052".parse().unwrap(); // Use different port for each test
    let config = ServiceHostConfig {
        db_path,
        session_manager_config: SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: steer_core::config::model::builtin::claude_3_5_sonnet_20241022(),
            auto_persist: true,
        },
        bind_addr,
        auth_storage: Arc::new(steer_core::test_utils::InMemoryAuthStorage::new()),
    };

    let mut service_host = ServiceHost::new(config).await.unwrap();
    service_host.start().await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let channel = Channel::from_shared(format!("http://{bind_addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    // Create session
    let mut grpc_client = AgentServiceClient::new(channel.clone());
    let create_req = CreateSessionRequest {
        workspace_config: Some(WorkspaceConfig {
            config: Some(steer_proto::agent::v1::workspace_config::Config::Local(
                steer_proto::agent::v1::LocalWorkspaceConfig {
                    path: workspace_path.to_string_lossy().to_string(),
                },
            )),
        }),
        ..Default::default()
    };

    let session_id = grpc_client
        .create_session(create_req)
        .await
        .unwrap()
        .into_inner()
        .session
        .unwrap()
        .id;

    // Get files via gRPC
    let list_req = ListFilesRequest {
        session_id: session_id.clone(),
        query: String::new(),
        max_results: 100,
    };

    let mut file_stream = grpc_client.list_files(list_req).await.unwrap().into_inner();
    let mut files: Vec<String> = Vec::new();

    while let Some(response) = file_stream.message().await.unwrap() {
        files.extend(response.paths);
    }

    info!("Got {} files for TUI test", files.len());

    // Test fuzzy matching locally (what TUI does internally)
    use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
    let matcher = SkimMatcherV2::default();

    // Search for "main"
    let query = "main";
    let mut results: Vec<(i64, String)> = files
        .iter()
        .filter_map(|file| {
            matcher
                .fuzzy_match(file, query)
                .map(|score| (score, file.clone()))
        })
        .collect();

    results.sort_by(|a, b| b.0.cmp(&a.0)); // Sort by score descending

    assert!(!results.is_empty(), "Should find files matching 'main'");
    assert!(
        results[0].1.contains("main"),
        "Top result should contain 'main'"
    );

    // Search for "rs" - should find all Rust files
    let query = "rs";
    let rust_files: Vec<String> = files
        .iter()
        .filter(|file| matcher.fuzzy_match(file, query).is_some())
        .cloned()
        .collect();

    assert!(rust_files.len() >= 3, "Should find at least 3 Rust files");

    // Test empty query returns all files (what happens when @ is first pressed)
    let all_files: Vec<String> = files.iter().take(20).cloned().collect(); // Limit to 20 like TUI does
    assert!(!all_files.is_empty(), "Should return files for empty query");

    service_host.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_workspace_changed_event_flow() {
    let _ = tracing_subscriber::fmt::try_init();

    // Setup
    let (_temp_dir, workspace_path) = setup_test_workspace().await.unwrap();

    // Start service
    let db_path = workspace_path.join("test_sessions.db");
    let bind_addr = "127.0.0.1:50053".parse().unwrap(); // Use different port for each test
    let config = ServiceHostConfig {
        db_path,
        session_manager_config: SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: steer_core::config::model::builtin::claude_3_5_sonnet_20241022(),
            auto_persist: true,
        },
        bind_addr,
        auth_storage: Arc::new(steer_core::test_utils::InMemoryAuthStorage::new()),
    };

    let mut service_host = ServiceHost::new(config).await.unwrap();
    service_host.start().await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let channel = Channel::from_shared(format!("http://{bind_addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    // Create session
    let mut grpc_client = AgentServiceClient::new(channel.clone());
    let create_req = CreateSessionRequest {
        workspace_config: Some(WorkspaceConfig {
            config: Some(steer_proto::agent::v1::workspace_config::Config::Local(
                steer_proto::agent::v1::LocalWorkspaceConfig {
                    path: workspace_path.to_string_lossy().to_string(),
                },
            )),
        }),
        ..Default::default()
    };

    let session_id = grpc_client
        .create_session(create_req)
        .await
        .unwrap()
        .into_inner()
        .session
        .unwrap()
        .id;

    // Start streaming events
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    // Subscribe to events
    let subscribe_msg = stream_session_request::Message::Subscribe(SubscribeRequest {
        event_types: vec![], // Empty means all events
        since_sequence: None,
    });

    let msg = steer_proto::agent::v1::StreamSessionRequest {
        session_id: session_id.clone(),
        message: Some(subscribe_msg),
    };

    // Create a bidirectional stream
    let outbound = tokio_stream::iter(vec![msg]);
    let response = grpc_client.stream_session(outbound).await.unwrap();
    let mut inbound = response.into_inner();

    // Spawn task to collect events
    let event_collector = tokio::spawn(async move {
        while let Some(event) = inbound.next().await {
            match event {
                Ok(server_event) => {
                    debug!("Received event: {:?}", server_event);
                    if let Some(
                        steer_proto::agent::v1::stream_session_response::Event::WorkspaceChanged(_),
                    ) = server_event.event
                    {
                        let _ = tx.send(()).await;
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Stream error: {e}");
                    break;
                }
            }
        }
    });

    // Execute a tool that modifies files to trigger WorkspaceChanged
    // Note: In practice, WorkspaceChanged only fires after mutating tools execute
    // For this test, we'll just verify the event stream works

    // Send a message that would trigger tool execution
    let _send_msg = stream_session_request::Message::SendMessage(SendMessageRequest {
        session_id: session_id.clone(),
        message: "Create a new file called test.txt with 'hello world' in it".to_string(),
        attachments: vec![],
    });

    // TODO: implement

    // Give it some time
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Cancel the event collector
    event_collector.abort();

    service_host.shutdown().await.unwrap();
}
