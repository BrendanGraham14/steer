//! EventProcessor trait and supporting types.
//!
//! This defines the core abstraction for the event processing pipeline.
//! Each processor handles a specific category of events and can be composed
//! into a pipeline for modular event handling.

use std::sync::Arc;

use crate::tui::state::{ChatStore, ToolCallRegistry};
use crate::tui::widgets::chat_list::ChatListState;
use conductor_core::api::Model;
use conductor_core::app::AppEvent;
use conductor_core::app::io::AppCommandSink;
use conductor_tools::schema::ToolCall;

/// Result of processing an event
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ProcessingResult {
    /// Event was handled successfully, continue to next processor
    Handled,
    /// Event was handled and no further processing needed
    ///
    HandledAndComplete,
    /// Event was not handled by this processor, try next one
    NotHandled,
    /// Event processing failed with an error
    Failed(String),
}

/// Context passed to event processors containing mutable access to TUI state
#[allow(dead_code)]
pub struct ProcessingContext<'a> {
    /// Chat store for adding/updating messages
    pub chat_store: &'a mut ChatStore,
    /// Chat list UI state (scroll, selection, etc.)
    pub chat_list_state: &'a mut ChatListState,
    /// Tool call registry for tracking tool lifecycle
    pub tool_registry: &'a mut ToolCallRegistry,
    /// Command sink for dispatching app commands
    pub command_sink: &'a Arc<dyn AppCommandSink>,
    /// Current processing state
    pub is_processing: &'a mut bool,
    /// Current progress message being displayed
    pub progress_message: &'a mut Option<String>,
    /// Current spinner animation state
    pub spinner_state: &'a mut usize,
    /// Current tool approval request
    pub current_tool_approval: &'a mut Option<ToolCall>,
    /// Current model being used
    pub current_model: &'a mut Model,
    /// Flag to indicate if messages were updated (for auto-scroll)
    pub messages_updated: &'a mut bool,
    /// Current thread ID (None until first message)
    pub current_thread: Option<uuid::Uuid>,
}

/// Trait for processing specific types of AppEvents
pub trait EventProcessor: Send + Sync {
    /// Priority for this processor (lower numbers run first)
    fn priority(&self) -> usize {
        100
    }

    /// Check if this processor can handle the given event
    fn can_handle(&self, event: &AppEvent) -> bool;

    /// Process the event with access to the processing context
    ///
    /// Processors should be deterministic and side-effect-free except through
    /// the provided context. They should not directly call external APIs or
    /// perform I/O operations.
    fn process(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> ProcessingResult;

    /// Name of this processor for debugging
    fn name(&self) -> &'static str;
}
