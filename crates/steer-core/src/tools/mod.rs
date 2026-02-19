pub mod agent_spawner_impl;
pub mod backend;
pub mod builtin_tool;
pub mod builtin_tools;
pub mod capability;
pub mod error;
pub mod execution_context;
pub mod executor;
pub mod factory;
pub mod mcp;
pub mod model_caller_impl;
pub mod registry;
pub mod resolver;
pub mod services;

pub use agent_spawner_impl::DefaultAgentSpawner;
pub use backend::{BackendMetadata, BackendRegistry, ToolBackend};
pub use builtin_tool::{BuiltinTool, BuiltinToolContext, BuiltinToolErased, BuiltinToolError};
pub use capability::Capabilities;
pub use error::ToolError;
pub use execution_context::ExecutionContext;
pub use executor::ToolExecutor;
pub use mcp::{McpBackend, McpError, McpTransport};
pub use model_caller_impl::DefaultModelCaller;
pub use registry::ToolRegistry;
pub use resolver::{BackendResolver, OverlayResolver, SessionMcpBackends};
pub use services::{
    AgentSpawner, ModelCaller, SubAgentConfig, SubAgentError, SubAgentResult, ToolServices,
};
pub use steer_tools::ToolSchema;
pub use steer_tools::tools::dispatch_agent::{
    DISPATCH_AGENT_TOOL_NAME, DispatchAgentParams, DispatchAgentTarget, WorkspaceTarget,
};
pub use steer_tools::tools::fetch::{FETCH_TOOL_NAME, FetchParams};

pub use factory::ToolSystemBuilder;
