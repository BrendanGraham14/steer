use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use steer_core::app::domain::action::{Action, McpServerState};
use steer_core::app::domain::types::SessionId;
use steer_grpc::client_api::{
    ClientEvent, CreateSessionParams, SessionPolicyOverrides, SessionToolConfig,
    WorkspaceConfig as ClientWorkspaceConfig,
};
use steer_grpc::{AgentClient, ServiceHost, ServiceHostConfig};
use steer_proto::agent::v1::{
    CreateSessionRequest, GetAuthProgressRequest, ListFilesRequest, StartAuthRequest,
    SubscribeSessionEventsRequest, WorkspaceConfig as ProtoWorkspaceConfig,
    agent_service_client::AgentServiceClient, auth_progress::State as AuthProgressState,
};
use tempfile::TempDir;
use tokio::time::{Duration, Instant, timeout_at};
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tracing::{debug, info};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Create a test workspace with some files
async fn setup_test_workspace() -> Result<(TempDir, PathBuf)> {
    let temp_dir = TempDir::new()?;
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
    .await?;

    tokio::fs::write(
        workspace_path.join("README.md"),
        "# Test Project\n\nThis is a test project for fuzzy finder testing.\n",
    )
    .await?;

    tokio::fs::write(
        workspace_path.join("src/main.rs"),
        r#"fn main() {
    println!("Hello, world!");
}
"#,
    )
    .await?;

    tokio::fs::write(
        workspace_path.join("src/lib.rs"),
        r"//! Test library
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
",
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
        r"#[test]
fn test_something() {
    assert_eq!(2 + 2, 4);
}
",
    )
    .await?;

    // Create a .gitignore file
    tokio::fs::write(workspace_path.join(".gitignore"), "target/\n*.swp\n").await?;

    Ok((temp_dir, workspace_path))
}

fn unused_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

#[tokio::test]
async fn test_tui_agent_service_file_listing() {
    // Initialize logging
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("off")),
        )
        .try_init();

    // Setup test workspace
    let (_temp_dir, workspace_path) = setup_test_workspace()
        .await
        .expect("Failed to setup test workspace");
    info!("Created test workspace at: {:?}", workspace_path);

    let db_path = workspace_path.join("test_sessions.db");
    let bind_addr = "127.0.0.1:50051".parse().unwrap();
    let config = ServiceHostConfig {
        db_path,
        bind_addr,
        auth_storage: Arc::new(steer_core::test_utils::InMemoryAuthStorage::new()),
        catalog_config: steer_core::catalog::CatalogConfig::default(),
        workspace_root: Some(workspace_path.clone()),
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
    let default_model = steer_core::config::model::builtin::claude_sonnet_4_5();
    let create_req = CreateSessionRequest {
        workspace_config: Some(ProtoWorkspaceConfig {
            config: Some(steer_proto::agent::v1::workspace_config::Config::Local(
                steer_proto::agent::v1::LocalWorkspaceConfig {
                    path: workspace_path.to_string_lossy().to_string(),
                },
            )),
        }),
        metadata: [("test".to_string(), "true".to_string())]
            .into_iter()
            .collect(),
        default_model: Some(steer_proto::agent::v1::ModelSpec {
            provider_id: default_model.provider.storage_key(),
            model_id: default_model.id.clone(),
        }),
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
async fn test_tui_auth_flow_poll_no_input() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("off")),
        )
        .try_init();

    let (_temp_dir, workspace_path) = setup_test_workspace()
        .await
        .expect("Failed to setup test workspace");

    let db_path = workspace_path.join("test_sessions_auth.db");
    let bind_addr = format!("127.0.0.1:{}", unused_port()?).parse().unwrap();
    let config = ServiceHostConfig {
        db_path,
        bind_addr,
        auth_storage: Arc::new(steer_core::test_utils::InMemoryAuthStorage::new()),
        catalog_config: steer_core::catalog::CatalogConfig::default(),
        workspace_root: Some(workspace_path.clone()),
    };

    let mut service_host = ServiceHost::new(config).await.unwrap();
    service_host.start().await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let channel = Channel::from_shared(format!("http://{bind_addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut grpc_client = AgentServiceClient::new(channel.clone());

    let start = grpc_client
        .start_auth(StartAuthRequest {
            provider_id: "anthropic".to_string(),
        })
        .await
        .unwrap()
        .into_inner();

    assert!(!start.flow_id.is_empty());

    let progress = grpc_client
        .get_auth_progress(GetAuthProgressRequest {
            flow_id: start.flow_id.clone(),
        })
        .await
        .unwrap()
        .into_inner()
        .progress
        .expect("progress response");

    assert!(!matches!(progress.state, Some(AuthProgressState::Error(_))));

    service_host.shutdown().await.unwrap();
    Ok(())
}

async fn wait_for_mcp_event(
    event_rx: &mut tokio::sync::mpsc::Receiver<ClientEvent>,
    server_name: &str,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = timeout_at(deadline, event_rx.recv()).await.map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out waiting for MCP state event",
            )
        })?;
        let Some(event) = event else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "event stream closed before receiving MCP state event",
            )
            .into());
        };

        if let ClientEvent::McpServerStateChanged {
            server_name: name, ..
        } = event
        {
            if name == server_name {
                return Ok(());
            }
        }
    }
}

#[tokio::test]
async fn test_agent_client_resubscribes_events_on_session_switch() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("off")),
        )
        .try_init();

    let (_temp_dir, workspace_path) = setup_test_workspace().await.unwrap();

    let db_path = workspace_path.join("test_sessions_resubscribe.db");
    let bind_addr = format!("127.0.0.1:{}", unused_port()?).parse().unwrap();
    let config = ServiceHostConfig {
        db_path,
        bind_addr,
        auth_storage: Arc::new(steer_core::test_utils::InMemoryAuthStorage::new()),
        catalog_config: steer_core::catalog::CatalogConfig::default(),
        workspace_root: Some(workspace_path.clone()),
    };

    let mut service_host = ServiceHost::new(config).await.unwrap();
    service_host.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let channel = Channel::from_shared(format!("http://{bind_addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let client = AgentClient::from_channel(channel).await.unwrap();

    let default_model = steer_core::config::model::builtin::claude_sonnet_4_5();
    let session_params = CreateSessionParams {
        workspace: ClientWorkspaceConfig::Local {
            path: workspace_path.clone(),
        },
        tool_config: SessionToolConfig::default(),
        primary_agent_id: None,
        policy_overrides: SessionPolicyOverrides::empty(),
        metadata: HashMap::new(),
        default_model: default_model.clone(),
    };

    let first_session_id = client.create_session(session_params).await.unwrap();
    client.subscribe_session_events().await.unwrap();
    let mut event_rx = client.subscribe_client_events().await.unwrap();

    let first_session = SessionId::parse(&first_session_id).expect("valid session id");
    service_host
        .runtime_handle()
        .dispatch_action(
            first_session,
            Action::McpServerStateChanged {
                session_id: first_session,
                server_name: "test-mcp-1".to_string(),
                state: McpServerState::Disconnected { error: None },
            },
        )
        .await
        .unwrap();
    wait_for_mcp_event(&mut event_rx, "test-mcp-1").await?;

    let session_params = CreateSessionParams {
        workspace: ClientWorkspaceConfig::Local {
            path: workspace_path.clone(),
        },
        tool_config: SessionToolConfig::default(),
        primary_agent_id: None,
        policy_overrides: SessionPolicyOverrides::empty(),
        metadata: HashMap::new(),
        default_model,
    };

    let second_session_id = client.create_session(session_params).await.unwrap();
    assert_ne!(first_session_id, second_session_id);

    client.subscribe_session_events().await.unwrap();

    let second_session = SessionId::parse(&second_session_id).expect("valid session id");
    service_host
        .runtime_handle()
        .dispatch_action(
            second_session,
            Action::McpServerStateChanged {
                session_id: second_session,
                server_name: "test-mcp-2".to_string(),
                state: McpServerState::Disconnected { error: None },
            },
        )
        .await
        .unwrap();
    wait_for_mcp_event(&mut event_rx, "test-mcp-2").await?;

    drop(event_rx);
    client.shutdown().await;

    service_host.shutdown().await.unwrap();
    Ok(())
}

#[tokio::test]
async fn test_workspace_changed_event_flow() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("off")),
        )
        .try_init();

    // Setup
    let (_temp_dir, workspace_path) = setup_test_workspace().await.unwrap();

    let db_path = workspace_path.join("test_sessions.db");
    let bind_addr = "127.0.0.1:50053".parse().unwrap();
    let config = ServiceHostConfig {
        db_path,
        bind_addr,
        auth_storage: Arc::new(steer_core::test_utils::InMemoryAuthStorage::new()),
        catalog_config: steer_core::catalog::CatalogConfig::default(),
        workspace_root: Some(workspace_path.clone()),
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
    let default_model = steer_core::config::model::builtin::claude_sonnet_4_5();
    let create_req = CreateSessionRequest {
        workspace_config: Some(ProtoWorkspaceConfig {
            config: Some(steer_proto::agent::v1::workspace_config::Config::Local(
                steer_proto::agent::v1::LocalWorkspaceConfig {
                    path: workspace_path.to_string_lossy().to_string(),
                },
            )),
        }),
        default_model: Some(steer_proto::agent::v1::ModelSpec {
            provider_id: default_model.provider.storage_key(),
            model_id: default_model.id.clone(),
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

    let subscribe_req = SubscribeSessionEventsRequest {
        session_id: session_id.clone(),
        since_sequence: None,
    };

    let response = grpc_client
        .subscribe_session_events(subscribe_req)
        .await
        .unwrap();
    let mut event_stream = response.into_inner();

    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let event_collector = tokio::spawn(async move {
        while let Some(event) = event_stream.next().await {
            match event {
                Ok(server_event) => {
                    debug!("Received event: {:?}", server_event);
                    if let Some(steer_proto::agent::v1::session_event::Event::WorkspaceChanged(_)) =
                        server_event.event
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

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    event_collector.abort();

    service_host.shutdown().await.unwrap();
}
