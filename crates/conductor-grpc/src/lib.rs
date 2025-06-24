pub mod grpc;
pub mod in_memory;
pub mod service_host;

// Re-export everything from grpc module at the top level for backward compatibility
pub use grpc::{agent, agent as proto, remote_workspace};
pub use grpc::{client_adapter::*, error::*, server::*};
pub use service_host::*;
