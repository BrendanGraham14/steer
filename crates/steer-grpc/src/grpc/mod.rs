pub mod client_adapter;
pub(crate) mod conversions;
pub mod error;
pub mod runtime_server;

#[cfg(test)]
mod conversion_tests;

pub use steer_proto::{agent::v1 as agent, remote_workspace::v1 as remote_workspace};

pub use agent as proto;

pub use client_adapter::*;
pub use error::*;
pub use runtime_server::RuntimeAgentService;

pub use conversions::{convert_todo_item_to_proto, convert_todo_write_file_operation_to_proto};
