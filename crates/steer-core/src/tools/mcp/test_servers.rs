//! Test MCP servers for integration testing

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::schemars;
use rmcp::{
    ErrorData, ServerHandler,
    model::{CallToolResult, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use std::sync::Arc;
use tokio::sync::Mutex;

// Additional imports for HTTP server
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, session::local::LocalSessionManager,
};

/// A simple test MCP service that provides basic tools
#[derive(Debug, Clone)]
pub struct TestMcpService {
    call_count: Arc<Mutex<u32>>,
    tool_router: ToolRouter<TestMcpService>,
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
    #[tool(description = "Echo back the input message")]
    async fn echo(
        &self,
        Parameters(EchoRequest { message }): Parameters<EchoRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        {
            let mut count = self.call_count.lock().await;
            *count += 1;
        }

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            message,
        )]))
    }

    #[tool(description = "Add two numbers together")]
    async fn add(
        &self,
        Parameters(AddRequest { a, b }): Parameters<AddRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        {
            let mut count = self.call_count.lock().await;
            *count += 1;
        }

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!("{}", a + b),
        )]))
    }

    #[tool(description = "Get the number of times tools have been called")]
    async fn get_call_count(&self) -> Result<CallToolResult, ErrorData> {
        let count = self.call_count.lock().await;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!("{}", *count),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for TestMcpService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            instructions: None,
            ..Default::default()
        }
    }
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
