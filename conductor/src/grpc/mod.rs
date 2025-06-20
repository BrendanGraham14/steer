pub mod client_adapter;
pub mod conversions;
pub mod error;
pub mod events;
pub mod server;

// Re-export protobuf modules from conductor-proto crate
pub use conductor_proto::{agent, remote_workspace};

// Export commonly used types from agent proto for backward compatibility
pub use agent as proto; // Keep this for existing code that uses proto::*

pub use client_adapter::*;
pub use error::*;
pub use events::*;
pub use server::*;
