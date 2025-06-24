//! Event processing pipeline for TUI.
//!
//! This module implements **Phase 3** of the TUI refactor: extracting the large
//! `handle_app_event` method into a composable pipeline of EventProcessors.
//!
//! Each processor handles a specific category of events (messages, tools, etc.)
//! and can be easily tested and modified independently.

pub mod pipeline;
pub mod processor;
pub mod processors;

pub use pipeline::EventPipeline;
pub use processor::{EventProcessor, ProcessingContext, ProcessingResult};
