//! Specific EventProcessor implementations.
//!
//! Each processor handles a specific category of events, extracted from the
//! original monolithic `handle_app_event` method.

pub mod processing_state;
pub mod message;
pub mod tool;
pub mod system;

pub use processing_state::ProcessingStateProcessor;
pub use message::MessageEventProcessor;
pub use tool::ToolEventProcessor;
pub use system::SystemEventProcessor;