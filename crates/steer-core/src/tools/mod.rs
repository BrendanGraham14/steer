// Publicly export the main types
pub mod backend;
pub mod execution_context;
pub mod local_backend;
pub mod mcp_backend;

pub use backend::{BackendMetadata, BackendRegistry, ToolBackend};
// Re-export tools types as the primary tool abstractions
pub use execution_context::ExecutionContext;
pub use local_backend::LocalBackend;
pub use mcp_backend::{McpBackend, McpTransport};
pub use steer_tools::{ToolError, ToolSchema};

// Export the remaining main-app specific tool modules
pub mod command_filter;
pub mod dispatch_agent;
pub mod fetch;

pub use dispatch_agent::{DISPATCH_AGENT_TOOL_NAME, DispatchAgentTool};
pub use fetch::{FETCH_TOOL_NAME, FetchTool};

#[cfg(test)]
mod mcp_backend_test;
#[cfg(test)]
mod mcp_test_servers;
