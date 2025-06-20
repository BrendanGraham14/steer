//! EventProcessor trait and supporting types.
//!
//! This defines the core abstraction for the event processing pipeline.
//! Each processor handles a specific category of events and can be composed
//! into a pipeline for modular event handling.

use tokio::sync::mpsc;

use crate::api::Model;
use crate::app::AppEvent;
use crate::app::command::AppCommand;
use crate::tui::state::{MessageStore, ToolCallRegistry};
use crate::tui::widgets::message_list::MessageListState;
use tools::schema::ToolCall;

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
    /// Message store for adding/updating messages
    pub messages: &'a mut MessageStore,
    /// Message list UI state (scroll, selection, etc.)
    pub message_list_state: &'a mut MessageListState,
    /// Tool call registry for tracking tool lifecycle
    pub tool_registry: &'a mut ToolCallRegistry,
    /// Command sender for dispatching app commands
    pub command_tx: &'a mpsc::Sender<AppCommand>,
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
}

impl<'a> ProcessingContext<'a> {
    /// Helper to get or create a tool message index
    pub fn get_or_create_tool_index(&mut self, id: &str, name_hint: Option<String>) -> usize {
        if let Some(idx) = self.tool_registry.get_message_index(id) {
            return idx;
        }

        // Build placeholder ToolCall
        let placeholder_call = ToolCall {
            id: id.to_string(),
            name: name_hint.unwrap_or_else(|| "unknown".to_string()),
            parameters: serde_json::Value::Null,
        };

        let message_content = crate::tui::widgets::message_list::MessageContent::Tool {
            id: id.to_string(),
            call: placeholder_call.clone(),
            result: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        self.messages.push(message_content);
        let idx = self.messages.len() - 1;
        self.tool_registry.set_message_index(id, idx);
        self.tool_registry.register_call(placeholder_call);

        idx
    }

    /// Helper to convert app::Message to MessageContent
    pub fn convert_message(&self, message: crate::app::Message) -> crate::tui::widgets::message_list::MessageContent {
        use crate::app::conversation::AssistantContent;
        use crate::tui::widgets::message_list::MessageContent;

        match message {
            crate::app::Message::User {
                content,
                timestamp,
                id,
            } => MessageContent::User {
                id,
                blocks: content,
                timestamp: chrono::DateTime::from_timestamp(timestamp as i64, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| timestamp.to_string()),
            },
            crate::app::Message::Assistant {
                content,
                timestamp,
                id,
            } => {
                // Always keep tool calls as part of Assistant messages
                // The Tool message will be handled separately
                MessageContent::Assistant {
                    id,
                    blocks: content,
                    timestamp: chrono::DateTime::from_timestamp(timestamp as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| timestamp.to_string()),
                }
            }
            crate::app::Message::Tool {
                tool_use_id,
                result,
                timestamp,
                id: _,
            } => {
                // Try to find the corresponding ToolCall that we cached earlier
                let tool_call = self.tool_registry
                    .get_tool_call(&tool_use_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        tracing::warn!(target:"tui.convert_message", "Tool message {} has no associated tool call info", tool_use_id);
                        ToolCall {
                            id: tool_use_id.clone(),
                            name: "unknown".to_string(),
                            parameters: serde_json::Value::Null,
                        }
                    });

                MessageContent::Tool {
                    id: tool_use_id,
                    call: tool_call,
                    result: Some(result),
                    timestamp: chrono::DateTime::from_timestamp(timestamp as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| timestamp.to_string()),
                }
            }
        }
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
