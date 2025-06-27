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
pub enum ProcessingResult {
    /// Event was handled successfully, continue to next processor
    Handled,
    /// Event was handled and no further processing needed
    HandledAndComplete,
    /// Event was not handled by this processor, try next one
    NotHandled,
    /// Event processing failed with an error
    Failed(String),
}

/// Context passed to event processors containing mutable access to TUI state
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

impl ProcessingContext<'_> {
    /// Helper to get or create a tool message index
    pub fn get_or_create_tool_index(&mut self, id: &str, name_hint: Option<String>) -> usize {
        // First, try to find existing tool message by id
        if let Some((idx, _)) = self.chat_store.iter().enumerate().find(|(_, item)| {
            if let crate::tui::model::ChatItem::Message(row) = item {
                if let conductor_core::app::conversation::Message::Tool { tool_use_id, .. } =
                    &row.inner
                {
                    tool_use_id == id
                } else {
                    false
                }
            } else {
                false
            }
        }) {
            return idx;
        }

        // Build placeholder ToolCall
        let placeholder_call = ToolCall {
            id: id.to_string(),
            name: name_hint.unwrap_or_else(|| "unknown".to_string()),
            parameters: serde_json::Value::Null,
        };

        // Create a placeholder Tool message
        let tool_msg = conductor_core::app::conversation::Message::Tool {
            id: crate::tui::model::generate_row_id(),
            tool_use_id: id.to_string(),
            result: conductor_core::app::conversation::ToolResult::Success {
                output: "Pending...".to_string(),
            },
            timestamp: chrono::Utc::now().timestamp() as u64,
            thread_id: self.current_thread.unwrap_or(uuid::Uuid::new_v4()),
            parent_message_id: None,
        };

        let idx = self.chat_store.add_message(tool_msg);
        self.tool_registry.set_message_index(id, idx);
        self.tool_registry.register_call(placeholder_call);

        idx
    }
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
