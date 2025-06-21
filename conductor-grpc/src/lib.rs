pub mod grpc;
pub mod service_host;
pub mod in_memory;

// Re-export everything from grpc module at the top level for backward compatibility
pub use grpc::{client_adapter::*, error::*, events::*, server::*};
pub use grpc::{agent, agent as proto, remote_workspace};
pub use service_host::*;