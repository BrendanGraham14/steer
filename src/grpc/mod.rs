pub mod server;
pub mod events;
pub mod client_adapter;

// Re-export the generated protobuf code
pub mod proto {
    tonic::include_proto!("coder.agent.v1");
}

pub use server::*;
pub use events::*;
pub use client_adapter::*;