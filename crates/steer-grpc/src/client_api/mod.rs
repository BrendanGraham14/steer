//! Client-facing API for Steer applications (TUI, IDE plugins, etc.).

mod command;
mod event;
mod types;

pub use command::{ApprovalDecision, ClientCommand};
pub use event::ClientEvent;
pub use types::*;
