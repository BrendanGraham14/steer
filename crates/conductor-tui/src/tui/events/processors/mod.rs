//! Specific EventProcessor implementations.
//!
//! Each processor handles a specific category of events, extracted from the
//! original monolithic `handle_app_event` method.

pub mod message;
pub mod processing_state;
pub mod system;
pub mod tool;

