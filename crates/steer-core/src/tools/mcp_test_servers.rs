//! Test MCP servers for integration testing

#[cfg(test)]
pub mod test_servers {
    use rmcp::handler::server::tool::Parameters;
    use rmcp::schemars;
    use rmcp::{ErrorData, ServerHandler, model::CallToolResult, tool_handler, tool_router};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // Additional imports for SSE and HTTP servers
    use rmcp::transport::sse_server::{SseServer, SseServerConfig};
    use rmcp::transport::streamable_http_server::{
        StreamableHttpService, session::local::LocalSessionManager,
    };

    /// A simple test MCP service that provides basic tools
    #[derive(Debug, Clone)]
    pub struct TestMcpService {
        call_count: Arc<Mutex<u32>>,
        tool_router: rmcp::handler::server::router::tool::ToolRouter<TestMcpService>,
    }

    impl TestMcpService {
        pub fn new() -> Self {
            Self {
                call_count: Arc::new(Mutex::new(0)),
                tool_router: Self::tool_router(),
            }
        }
    }

    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct EchoRequest {
        #[schemars(description = "Message to echo back")]
        pub message: String,
    }

    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct AddRequest {
        #[schemars(description = "First number to add")]
        pub a: f64,
        #[schemars(description = "Second number to add")]
        pub b: f64,
    }

    #[tool_router]
    impl TestMcpService {
        #[rmcp::tool(description = "Echo back the input message")]
        async fn echo(
            &self,
            Parameters(EchoRequest { message }): Parameters<EchoRequest>,
        ) -> Result<CallToolResult, ErrorData> {
            // Increment call count
            {
                let mut count = self.call_count.lock().await;
                *count += 1;
            }
            Ok(CallToolResult::success(vec![rmcp::model::Content::text(
                message,
            )]))
        }

        #[rmcp::tool(description = "Add two numbers together")]
        async fn add(
            &self,
            Parameters(AddRequest { a, b }): Parameters<AddRequest>,
        ) -> Result<CallToolResult, ErrorData> {
            // Increment call count
            {
                let mut count = self.call_count.lock().await;
                *count += 1;
            }
            Ok(CallToolResult::success(vec![rmcp::model::Content::text(
                format!("{}", a + b),
            )]))
        }

        #[rmcp::tool(description = "Get the number of times tools have been called")]
        async fn get_call_count(&self) -> Result<CallToolResult, ErrorData> {
            let count = self.call_count.lock().await;
            Ok(CallToolResult::success(vec![rmcp::model::Content::text(
                format!("{}", *count),
            )]))
        }
    }

    #[tool_handler]
    impl ServerHandler for TestMcpService {
        fn get_info(&self) -> rmcp::model::ServerInfo {
            use rmcp::model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo};
            ServerInfo {
                protocol_version: ProtocolVersion::V_2024_11_05,
                capabilities: ServerCapabilities::builder().enable_tools().build(),
                server_info: Implementation {
                    name: "test-mcp-service".to_string(),
                    version: "1.0.0".to_string(),
                },
                instructions: None,
            }
        }
    }

    /// Start an SSE server for testing
    pub async fn start_sse_server(
        bind_addr: String,
    ) -> Result<tokio_util::sync::CancellationToken, Box<dyn std::error::Error + Send + Sync>> {
        let config = SseServerConfig {
            bind: bind_addr.parse()?,
            sse_path: "/sse".to_string(),
            post_path: "/message".to_string(),
            ct: tokio_util::sync::CancellationToken::new(),
            sse_keep_alive: None,
        };

        let (sse_server, router) = SseServer::new(config);
        let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;

        let ct = sse_server.config.ct.child_token();
        let server = axum::serve(listener, router).with_graceful_shutdown(async move {
            ct.cancelled().await;
        });

        tokio::spawn(async move {
            if let Err(e) = server.await {
                tracing::error!(error = %e, "sse server shutdown with error");
            }
        });

        let server_ct = sse_server.with_service(TestMcpService::new);
        Ok(server_ct)
    }

    /// Start an HTTP streamable server for testing
    pub async fn start_http_server(
        bind_addr: String,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let service = StreamableHttpService::new(
            || Ok(TestMcpService::new()),
            LocalSessionManager::default().into(),
            Default::default(),
        );

        let router = axum::Router::new().nest_service("/mcp", service);
        let tcp_listener = tokio::net::TcpListener::bind(bind_addr).await?;

        tokio::spawn(async move {
            axum::serve(tcp_listener, router).await.unwrap();
        });

        // Give the server a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        Ok(())
    }
}
