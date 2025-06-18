//! MessageViewModel – phase-2 extraction (minimal).
//!
//! Holds the canonical `MessageStore` (data) **and** the per-view `MessageListState`.
//! For phase-2 we expose the same field names (`messages`, `message_list_state`) that
//! `tui::Tui` previously carried so downstream call-sites can be migrated with a
//! simple prefix change ( `self.message_list_state` → `self.view_model.message_list_state`,
//! `self.messages` → `self.view_model.messages` ).  No behaviour change.

use crate::app::conversation::AssistantContent;
use crate::tui::widgets::message_list::{MessageContent, MessageListState};
use super::message_store::MessageStore;
use super::tool_registry::ToolCallRegistry;

#[derive(Debug)]
pub struct MessageViewModel {
    /// Ordered list of message UI models
    pub messages: MessageStore,
    /// Scroll/selection/cache state for the list widget
    pub message_list_state: MessageListState,
    /// Centralized tool call lifecycle tracking
    pub tool_registry: ToolCallRegistry,
}

impl MessageViewModel {
    pub fn new() -> Self {
        Self {
            messages: MessageStore::new(),
            message_list_state: MessageListState::new(),
            tool_registry: ToolCallRegistry::new(),
        }
    }

    #[inline]
    pub fn as_slice(&self) -> &[MessageContent] {
        self.messages.as_slice()
    }

    /// Add a message to the view model, handling tool registry coordination automatically
    pub fn add_message(&mut self, content: MessageContent) {
        // Handle tool call registration for assistant messages
        if let MessageContent::Assistant { blocks, .. } = &content {
            for block in blocks {
                if let AssistantContent::ToolCall { tool_call } = block {
                    self.tool_registry.register_call(tool_call.clone());
                }
            }
        }

        // Add the message and get its index
        let message_index = self.messages.len();
        self.messages.push(content.clone());

        // For tool messages, set the message index in the registry
        if let MessageContent::Tool { id, call, .. } = &content {
            self.tool_registry.register_call(call.clone());
            self.tool_registry.set_message_index(id, message_index);
        }
    }

    /// Add multiple messages efficiently 
    pub fn add_messages(&mut self, messages: Vec<MessageContent>) {
        for message in messages {
            self.add_message(message);
        }
    }
}
