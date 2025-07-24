mod backend;
mod error;

pub use backend::{McpBackend, McpTransport};
pub use error::McpError;

#[cfg(test)]
mod backend_test;
#[cfg(test)]
mod test_servers;
