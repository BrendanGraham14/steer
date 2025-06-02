// Publicly export the error and trait types
pub mod backend;
pub mod error;
pub mod execution_context;
pub mod local_backend;
pub mod traits;

pub use backend::{BackendMetadata, BackendRegistry, ToolBackend};
pub use error::ToolError;
pub use execution_context::{AuthMethod, ExecutionContext, ExecutionEnvironment, VolumeMount};
pub use local_backend::LocalBackend;
pub use traits::Tool;

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
