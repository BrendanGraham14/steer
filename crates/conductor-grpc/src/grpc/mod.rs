pub mod client_adapter;
pub mod conversions;
pub mod error;
pub mod server;
pub mod session_manager_ext;

#[cfg(test)]
mod conversion_tests;

// Re-export protobuf modules from conductor-proto crate
pub use conductor_proto::{agent::v1 as agent, remote_workspace::v1 as remote_workspace};

// Export commonly used types from agent proto for backward compatibility
pub use agent as proto; // Keep this for existing code that uses proto::*

pub use client_adapter::*;
pub use error::*;
pub use server::*;
pub use session_manager_ext::SessionManagerExt;
