pub mod client_adapter;
pub mod conversions;
pub mod error;
pub mod events;
pub mod server;

// Re-export the generated protobuf code for agent service
pub mod agent {
    tonic::include_proto!("conductor.agent.v1");
}

// Re-export the generated protobuf code for remote workspace service
pub mod remote_workspace {
    tonic::include_proto!("conductor.remote_workspace.v1");
}

// Export commonly used types from agent proto for backward compatibility
pub use agent as proto; // Keep this for existing code that uses proto::*

pub use client_adapter::*;
pub use error::*;
pub use events::*;
pub use server::*;
