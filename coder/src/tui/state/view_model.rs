//! MessageViewModel – phase-2 extraction (minimal).
//!
//! Holds the canonical `MessageStore` (data) **and** the per-view `MessageListState`.
//! For phase-2 we expose the same field names (`messages`, `message_list_state`) that
//! `tui::Tui` previously carried so downstream call-sites can be migrated with a
//! simple prefix change ( `self.message_list_state` → `self.view_model.message_list_state`,
//! `self.messages` → `self.view_model.messages` ).  No behaviour change.

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
}
