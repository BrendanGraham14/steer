// Publicly export the main types
pub mod backend;
pub mod execution_context;
pub mod local_backend;

pub use backend::{BackendMetadata, BackendRegistry, ToolBackend};
// Re-export coder_tools types as the primary tool abstractions
pub use coder_tools::{Tool, ToolError};
pub use execution_context::{AuthMethod, ExecutionContext, ExecutionEnvironment, VolumeMount};
pub use local_backend::LocalBackend;

// Export the remaining main-app specific tool modules
pub mod command_filter;
pub mod dispatch_agent;
pub mod fetch;

pub use dispatch_agent::DispatchAgentTool;
pub use fetch::FetchTool;
