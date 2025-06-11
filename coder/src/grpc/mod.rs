pub mod client_adapter;
pub mod conversions;
pub mod events;
pub mod server;

// Re-export the generated protobuf code for agent service
pub mod agent {
    tonic::include_proto!("coder.agent.v1");
}

// Re-export the generated protobuf code for remote backend service
pub mod remote_backend {
    tonic::include_proto!("coder.remote_backend.v1");
}

// Export commonly used types from agent proto for backward compatibility
pub use agent as proto; // Keep this for existing code that uses proto::*

pub use client_adapter::*;
pub use events::*;
pub use server::*;
