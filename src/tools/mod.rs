// Publicly export the error and trait types
pub mod error;
pub mod traits;
pub mod execution_context;
pub mod backend;
pub mod local_backend;

pub use error::ToolError;
pub use traits::Tool;
pub use execution_context::{ExecutionContext, ExecutionEnvironment, AuthMethod, TraceContext, VolumeMount};
pub use backend::{ToolBackend, BackendRegistry, BackendMetadata};
pub use local_backend::LocalBackend;

// Export the individual tool modules
// These modules will now contain the Tool implementations
pub mod bash;
pub mod command_filter;
pub mod dispatch_agent;
pub mod edit;
pub mod fetch;
pub mod glob_tool;
pub mod grep_tool;
pub mod ls;
pub mod replace;
pub mod todo;
pub mod view;
