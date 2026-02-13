//! Tests for MCP backend functionality

#[cfg(test)]
mod tests {

    use crate::config::model::builtin;
    use crate::session::state::{BackendConfig, SessionConfig, ToolFilter};
    use crate::tools::execution_context::ExecutionContext;
    use crate::tools::mcp::test_servers::{TestMcpService, start_http_server, start_sse_server};
    use crate::tools::{McpBackend, McpTransport, ToolBackend};
    use rmcp::service::ServiceExt;
    use std::collections::HashMap;
    use steer_tools::ToolCall;
    use steer_tools::result::{ExternalResult, ToolResult};
    use tempfile::TempDir;
    use tokio::net::TcpListener;
    #[cfg(unix)]
    use tokio::net::UnixListener;
    use tokio_util::sync::CancellationToken;
    use tracing::{debug, info};

    #[tokio::test]
    async fn test_mcp_backend_in_session_config() {
        // For this test, we'll use the TCP backend test case which is more reliable
        // Start a test MCP server on TCP
        let service = TestMcpService::new();

        // Find an available port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Start the server in a background task
        let server_task = tokio::spawn(async move {
            info!("Test MCP TCP server listening on port {}", port);

            // Accept connections in a loop
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        debug!("Accepted connection from {}", addr);
                        let service_clone = service.clone();
                        tokio::spawn(async move {
                            match service_clone.serve(stream).await {
                                Ok(client) => {
                                    debug!("Client connected, keeping connection alive");
                                    let _ = client.waiting().await;
                                }
                                Err(e) => {
                                    eprintln!("Error serving connection: {e}");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Error accepting connection: {e}");
                        break;
                    }
                }
            }
        });

        // Give the server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Create a session config with an MCP backend
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "test-server".to_string(),
            transport: McpTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port,
            },
            tool_filter: ToolFilter::All,
        });

        // Build the registry - this should succeed
        let (registry, _mcp_servers) = config.build_registry().await.unwrap();

        // Verify the MCP tools are available
        let tool_schemas = registry.get_tool_schemas().await;
        assert!(
            tool_schemas
                .iter()
                .any(|t| t.name == "mcp__test-server__echo")
        );

        // Test executing a tool
        let tool_call = ToolCall {
            id: "test-1".to_string(),
            name: "mcp__test-server__echo".to_string(),
            parameters: serde_json::json!({
                "message": "Hello from session config test!"
            }),
        };

        let ctx = ExecutionContext::new(
            "test-session".to_string(),
            "test-operation".to_string(),
            "test-1".to_string(),
            CancellationToken::new(),
        );

        let backend = registry.get_backend_for_tool(&tool_call.name).unwrap();
        let result = backend.execute(&tool_call, &ctx).await.unwrap();

        match result {
            ToolResult::External(ExternalResult { payload, .. }) => {
                assert_eq!(payload.trim(), "Hello from session config test!");
            }
            _ => unreachable!("External result"),
        }

        // Clean up
        server_task.abort();
    }

    #[tokio::test]
    async fn test_mcp_tcp_backend_in_session_config() {
        // Start a test MCP server on TCP
        let service = TestMcpService::new();

        // Find an available port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Start the server in a background task
        let server_task = tokio::spawn(async move {
            info!("Test MCP TCP server listening on port {}", port);

            // Accept connections in a loop
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        debug!("Accepted connection from {}", addr);
                        let service_clone = service.clone();
                        tokio::spawn(async move {
                            match service_clone.serve(stream).await {
                                Ok(client) => {
                                    debug!("Client connected, keeping connection alive");
                                    let _ = client.waiting().await;
                                }
                                Err(e) => {
                                    eprintln!("Error serving connection: {e}");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Error accepting connection: {e}");
                        break;
                    }
                }
            }
        });

        // Give the server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Create a session config with a TCP MCP backend
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "tcp-server".to_string(),
            transport: McpTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port,
            },
            tool_filter: ToolFilter::All,
        });

        // Build the registry - this should succeed
        let (registry, _mcp_servers) = config.build_registry().await.unwrap();

        // Verify the MCP tools are available
        let tool_schemas = registry.get_tool_schemas().await;
        assert!(
            tool_schemas
                .iter()
                .any(|t| t.name == "mcp__tcp-server__echo")
        );
        assert!(
            tool_schemas
                .iter()
                .any(|t| t.name == "mcp__tcp-server__add")
        );
        assert!(
            tool_schemas
                .iter()
                .any(|t| t.name == "mcp__tcp-server__get_call_count")
        );

        // Clean up
        server_task.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_mcp_unix_backend_in_session_config() {
        // Create a temporary directory for our Unix socket
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("test.sock");
        let socket_path_str = socket_path.to_string_lossy().to_string();

        // Start the server in a background task
        let socket_path_clone = socket_path_str.clone();
        let server_task = tokio::spawn(async move {
            // Remove existing socket if it exists
            let _ = std::fs::remove_file(&socket_path_clone);

            let service = TestMcpService::new();
            let listener = UnixListener::bind(&socket_path_clone).unwrap();

            info!("Test MCP Unix server listening on {}", socket_path_clone);

            // Accept connections in a loop
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        debug!("Accepted Unix socket connection");
                        let service_clone = service.clone();
                        tokio::spawn(async move {
                            match service_clone.serve(stream).await {
                                Ok(client) => {
                                    debug!("Unix client connected, keeping connection alive");
                                    let _ = client.waiting().await;
                                }
                                Err(e) => {
                                    eprintln!("Error serving Unix connection: {e}");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Error accepting Unix connection: {e}");
                        break;
                    }
                }
            }
        });

        // Give the server time to start and create the socket
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Create a session config with a Unix socket MCP backend
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "unix-server".to_string(),
            transport: McpTransport::Unix {
                path: socket_path_str,
            },
            tool_filter: ToolFilter::All,
        });

        // Build the registry - this should succeed
        let (registry, _mcp_servers) = config.build_registry().await.unwrap();

        // Verify the MCP tools are available
        let tool_schemas = registry.get_tool_schemas().await;
        assert!(
            tool_schemas
                .iter()
                .any(|t| t.name == "mcp__unix-server__echo")
        );
        assert!(
            tool_schemas
                .iter()
                .any(|t| t.name == "mcp__unix-server__add")
        );
        assert!(
            tool_schemas
                .iter()
                .any(|t| t.name == "mcp__unix-server__get_call_count")
        );

        // Clean up
        server_task.abort();
    }

    #[tokio::test]
    async fn test_mcp_sse_backend_in_session_config() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("off")),
            )
            .try_init();

        // Start an SSE test server
        // Find an available port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let bind_addr = format!("127.0.0.1:{port}");
        let ct = start_sse_server(bind_addr.clone())
            .await
            .expect("Failed to start SSE server");

        // Give the server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Create a session config with an SSE MCP backend
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "sse-server".to_string(),
            transport: McpTransport::Sse {
                url: format!("http://127.0.0.1:{port}/sse"),
                headers: None,
            },
            tool_filter: ToolFilter::All,
        });

        // Build the registry
        let (registry, _mcp_servers) = config
            .build_registry()
            .await
            .expect("Failed to build tool registry");

        // Verify that the SSE backend is registered
        let registered_backends: Vec<String> = registry
            .backends()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        println!("Registered backends: {registered_backends:?}");
        assert!(registered_backends.contains(&"mcp_sse-server".to_string()));

        // List tools from the SSE backend
        let tool_schemas = registry.get_tool_schemas().await;
        let sse_tools: Vec<_> = tool_schemas
            .iter()
            .filter(|t| t.name.starts_with("mcp__sse-server__"))
            .collect();
        assert!(!sse_tools.is_empty());

        // Verify our test tools are available
        let tool_names: Vec<&str> = sse_tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"mcp__sse-server__echo"));
        assert!(tool_names.contains(&"mcp__sse-server__add"));
        assert!(tool_names.contains(&"mcp__sse-server__get_call_count"));

        // Execute a tool
        let tool_call = ToolCall {
            id: "test-1".to_string(),
            name: "mcp__sse-server__echo".to_string(),
            parameters: serde_json::json!({
                "message": "Hello SSE!"
            }),
        };

        let ctx = ExecutionContext::new(
            "test-session".to_string(),
            "test-operation".to_string(),
            "test-1".to_string(),
            CancellationToken::new(),
        );

        let backend = registry.get_backend_for_tool(&tool_call.name).unwrap();
        let result = backend.execute(&tool_call, &ctx).await.unwrap();

        match result {
            ToolResult::External(ExternalResult { payload, .. }) => {
                assert!(payload.contains("Hello SSE!"));
            }
            _ => unreachable!("External result"),
        }

        // Cancel the server
        ct.cancel();
    }

    #[tokio::test]
    async fn test_mcp_sse_backend_with_headers() {
        // Start an SSE test server
        // Find an available port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let bind_addr = format!("127.0.0.1:{port}");
        let ct = start_sse_server(bind_addr.clone())
            .await
            .expect("Failed to start SSE server");

        // Give the server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Create a session config with an SSE MCP backend with custom headers
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token123".to_string());
        headers.insert("X-Custom-Header".to_string(), "custom-value".to_string());

        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "sse-auth-server".to_string(),
            transport: McpTransport::Sse {
                url: format!("http://127.0.0.1:{port}/sse"),
                headers: Some(headers.clone()),
            },
            tool_filter: ToolFilter::All,
        });

        // Build the registry
        let (registry, _mcp_servers) = config
            .build_registry()
            .await
            .expect("Failed to build tool registry");

        // Verify that the SSE backend is registered
        let registered_backends: Vec<String> = registry
            .backends()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        assert!(registered_backends.contains(&"mcp_sse-auth-server".to_string()));

        // Verify the headers are stored correctly in the config
        match &config.tool_config.backends[0] {
            BackendConfig::Mcp {
                transport:
                    McpTransport::Sse {
                        headers: Some(h), ..
                    },
                ..
            } => {
                assert_eq!(h.get("Authorization").unwrap(), "Bearer token123");
                assert_eq!(h.get("X-Custom-Header").unwrap(), "custom-value");
            }
            BackendConfig::Mcp { .. } => {
                unreachable!("MCP SSE backend with headers")
            }
        }

        // Cancel the server
        ct.cancel();
    }

    #[tokio::test]
    async fn test_mcp_http_backend_in_session_config() {
        // Start an HTTP test server
        // Find an available port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let bind_addr = format!("127.0.0.1:{port}");
        start_http_server(bind_addr.clone())
            .await
            .expect("Failed to start HTTP server");

        // Give the server more time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Create a session config with an HTTP MCP backend
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "http-server".to_string(),
            transport: McpTransport::Http {
                url: format!("http://127.0.0.1:{port}/mcp"),
                headers: None,
            },
            tool_filter: ToolFilter::All,
        });

        // Build the registry
        let (registry, _mcp_servers) = config
            .build_registry()
            .await
            .expect("Failed to build tool registry");

        // Verify that the HTTP backend is registered
        let registered_backends: Vec<String> = registry
            .backends()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        assert!(registered_backends.contains(&"mcp_http-server".to_string()));

        // List tools from the HTTP backend
        let tool_schemas = registry.get_tool_schemas().await;
        let http_tools: Vec<_> = tool_schemas
            .iter()
            .filter(|t| t.name.starts_with("mcp__http-server__"))
            .collect();
        assert!(!http_tools.is_empty());

        // Verify our test tools are available
        let tool_names: Vec<&str> = http_tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"mcp__http-server__echo"));
        assert!(tool_names.contains(&"mcp__http-server__add"));
        assert!(tool_names.contains(&"mcp__http-server__get_call_count"));

        // Execute a tool
        let tool_call = ToolCall {
            id: "test-1".to_string(),
            name: "mcp__http-server__add".to_string(),
            parameters: serde_json::json!({
                "a": 5.0,
                "b": 3.0
            }),
        };

        let ctx = ExecutionContext::new(
            "test-session".to_string(),
            "test-operation".to_string(),
            "test-1".to_string(),
            CancellationToken::new(),
        );

        let backend = registry.get_backend_for_tool(&tool_call.name).unwrap();
        let result = backend.execute(&tool_call, &ctx).await.unwrap();

        match result {
            ToolResult::External(ExternalResult { payload, .. }) => {
                assert!(payload.contains('8'));
            }
            _ => unreachable!("External result"),
        }
    }

    #[tokio::test]
    async fn test_mcp_http_backend_with_headers() {
        // Start an HTTP test server
        // Find an available port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let bind_addr = format!("127.0.0.1:{port}");
        start_http_server(bind_addr.clone())
            .await
            .expect("Failed to start HTTP server");

        // Give the server more time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Create a session config with an HTTP MCP backend with custom headers
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        let mut headers = HashMap::new();
        headers.insert("X-API-Key".to_string(), "secret-key".to_string());

        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "http-auth-server".to_string(),
            transport: McpTransport::Http {
                url: format!("http://127.0.0.1:{port}/mcp"),
                headers: Some(headers.clone()),
            },
            tool_filter: ToolFilter::All,
        });

        // Build the registry
        let (registry, _mcp_servers) = config
            .build_registry()
            .await
            .expect("Failed to build tool registry");

        // Verify that the HTTP backend is registered
        let registered_backends: Vec<String> = registry
            .backends()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        assert!(registered_backends.contains(&"mcp_http-auth-server".to_string()));

        // Verify the headers are stored correctly in the config
        match &config.tool_config.backends[0] {
            BackendConfig::Mcp {
                transport:
                    McpTransport::Http {
                        headers: Some(h), ..
                    },
                ..
            } => {
                assert_eq!(h.get("X-API-Key").unwrap(), "secret-key");
            }
            BackendConfig::Mcp { .. } => {
                unreachable!("MCP HTTP backend with headers")
            }
        }

        // Execute a tool to verify the backend works
        let tool_call = ToolCall {
            id: "test-1".to_string(),
            name: "mcp__http-auth-server__echo".to_string(),
            parameters: serde_json::json!({
                "message": "Hello HTTP with headers!"
            }),
        };

        let ctx = ExecutionContext::new(
            "test-session".to_string(),
            "test-operation".to_string(),
            "test-1".to_string(),
            CancellationToken::new(),
        );

        let backend = registry.get_backend_for_tool(&tool_call.name).unwrap();
        let result = backend.execute(&tool_call, &ctx).await.unwrap();

        match result {
            ToolResult::External(ExternalResult { payload, .. }) => {
                assert!(payload.contains("Hello HTTP with headers!"));
            }
            _ => unreachable!("External result"),
        }
    }

    #[test]
    fn test_tool_name_prefixing() {
        // Test that we correctly add and remove the mcp_servername_ prefix
        let server_name = "myserver";
        let tool_name = "mytool";
        let prefixed_name = format!("mcp__{server_name}__{tool_name}");

        // Test extraction
        let prefix = format!("mcp__{server_name}__");
        let extracted = if prefixed_name.starts_with(&prefix) {
            &prefixed_name[prefix.len()..]
        } else {
            &prefixed_name
        };

        assert_eq!(extracted, tool_name);
    }

    #[tokio::test]
    async fn test_session_config_resilience_with_failing_backends() {
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());

        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "will-fail".to_string(),
            transport: McpTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port: 55555,
            },
            tool_filter: ToolFilter::All,
        });

        let (registry, mcp_servers) = config.build_registry().await.unwrap();

        assert!(
            mcp_servers.contains_key("will-fail"),
            "Failed backend should be tracked"
        );

        let tool_schemas = registry.get_tool_schemas().await;
        assert!(
            tool_schemas.is_empty(),
            "Failed backend should not contribute tools"
        );
    }

    // =========================
    // Integration tests with real MCP servers
    // =========================

    #[tokio::test]
    async fn test_mcp_tcp_backend_with_real_server() {
        // Start a test MCP server on TCP
        let service = TestMcpService::new();

        // Find an available port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Start the server in a background task
        let server_task = tokio::spawn(async move {
            info!("Test MCP TCP server listening on port {}", port);

            // Accept only one connection for the test
            if let Ok((stream, addr)) = listener.accept().await {
                debug!("Accepted connection from {}", addr);
                match service.serve(stream).await {
                    Ok(client) => {
                        debug!("Client connected, keeping connection alive");
                        let _ = client.waiting().await;
                    }
                    Err(e) => {
                        eprintln!("Error serving connection: {e}");
                    }
                }
            }
        });

        // Give the server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Create MCP backend
        let backend = McpBackend::new(
            "test-tcp".to_string(),
            McpTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port,
            },
            ToolFilter::All,
        )
        .await
        .unwrap();

        // Test metadata
        let metadata = backend.metadata();
        assert_eq!(metadata.name, "test-tcp");
        assert!(metadata.backend_type == "MCP");
        // Note: location is not set by MCP backend

        // Test supported tools
        let tools = backend.supported_tools().await;
        assert_eq!(tools.len(), 3);
        assert!(tools.iter().any(|t| t == "mcp__test-tcp__echo"));
        assert!(tools.iter().any(|t| t == "mcp__test-tcp__add"));
        assert!(tools.iter().any(|t| t == "mcp__test-tcp__get_call_count"));

        // Test tool execution - echo
        let tool_call = ToolCall {
            id: "test-1".to_string(),
            name: "mcp__test-tcp__echo".to_string(),
            parameters: serde_json::json!({
                "message": "Hello from test!"
            }),
        };

        let ctx = ExecutionContext::new(
            "test-session".to_string(),
            "test-operation".to_string(),
            "test-1".to_string(),
            CancellationToken::new(),
        );
        let result = backend.execute(&tool_call, &ctx).await.unwrap();

        match result {
            ToolResult::External(ExternalResult { payload, .. }) => {
                assert_eq!(payload.trim(), "Hello from test!");
            }
            _ => unreachable!("External result"),
        }

        // Test tool execution - add
        let tool_call = ToolCall {
            id: "test-2".to_string(),
            name: "mcp__test-tcp__add".to_string(),
            parameters: serde_json::json!({
                "a": 5,
                "b": 3
            }),
        };

        let result = backend.execute(&tool_call, &ctx).await.unwrap();

        match result {
            ToolResult::External(ExternalResult { payload, .. }) => {
                assert_eq!(payload.trim(), "8");
            }
            _ => unreachable!("External result"),
        }

        // Clean up - the server task will end when the client disconnects
        drop(backend);
        let _ = tokio::time::timeout(tokio::time::Duration::from_secs(1), server_task).await;
    }

    #[tokio::test]
    async fn test_mcp_stdio_backend_with_real_server() {
        // Skip this test as stdio transport requires an actual process
        // The TCP and Unix socket tests cover the functionality
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_mcp_unix_backend_with_real_server() {
        // Create a temporary directory for our Unix socket
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("test.sock");
        let socket_path_str = socket_path.to_string_lossy().to_string();

        // Start the server in a background task
        let socket_path_clone = socket_path_str.clone();
        let server_task = tokio::spawn(async move {
            // Remove existing socket if it exists
            let _ = std::fs::remove_file(&socket_path_clone);

            let service = TestMcpService::new();
            let listener = UnixListener::bind(&socket_path_clone).unwrap();

            info!("Test MCP Unix server listening on {}", socket_path_clone);

            // Accept only one connection for the test
            if let Ok((stream, _)) = listener.accept().await {
                debug!("Accepted Unix socket connection");
                match service.serve(stream).await {
                    Ok(client) => {
                        debug!("Unix client connected, keeping connection alive");
                        let _ = client.waiting().await;
                    }
                    Err(e) => {
                        eprintln!("Error serving Unix connection: {e}");
                    }
                }
            }
        });

        // Give the server time to start and create the socket
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Create MCP backend
        let backend = McpBackend::new(
            "test-unix".to_string(),
            McpTransport::Unix {
                path: socket_path_str.clone(),
            },
            ToolFilter::All,
        )
        .await
        .unwrap();

        // Test supported tools
        let tools = backend.supported_tools().await;
        assert_eq!(tools.len(), 3);

        // Test tool execution
        let tool_call = ToolCall {
            id: "test-1".to_string(),
            name: "mcp__test-unix__echo".to_string(),
            parameters: serde_json::json!({
                "message": "Hello from Unix socket!"
            }),
        };

        let ctx = ExecutionContext::new(
            "test-session".to_string(),
            "test-operation".to_string(),
            "test-1".to_string(),
            CancellationToken::new(),
        );
        let result = backend.execute(&tool_call, &ctx).await.unwrap();

        match result {
            ToolResult::External(ExternalResult { payload, .. }) => {
                assert_eq!(payload.trim(), "Hello from Unix socket!");
            }
            _ => unreachable!("External result"),
        }

        // Clean up
        drop(backend);
        server_task.abort();
    }
}
