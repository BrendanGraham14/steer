//! MessageViewModel – phase-2 extraction (minimal).
//!
//! Holds the canonical `MessageStore` (data) **and** the per-view `MessageListState`.
//! For phase-2 we expose the same field names (`messages`, `message_list_state`) that
//! `tui::Tui` previously carried so downstream call-sites can be migrated with a
//! simple prefix change ( `self.message_list_state` → `self.view_model.message_list_state`,
//! `self.messages` → `self.view_model.messages` ).  No behaviour change.

use super::content_cache::ContentCache;
use super::message_store::MessageStore;
use super::tool_registry::ToolCallRegistry;
use crate::app::conversation::AssistantContent;
use crate::tui::widgets::message_list::{MessageContent, MessageListState, ViewMode};
use std::sync::{Arc, RwLock};

#[derive(Debug)]
pub struct MessageViewModel {
    /// Ordered list of message UI models
    pub messages: MessageStore,
    /// Scroll/selection/cache state for the list widget
    pub message_list_state: MessageListState,
    /// Centralized tool call lifecycle tracking
    pub tool_registry: ToolCallRegistry,
    /// Content rendering cache for performance
    content_cache: Arc<RwLock<ContentCache>>,
}

impl Default for MessageViewModel {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageViewModel {
    pub fn new() -> Self {
        Self {
            messages: MessageStore::new(),
            message_list_state: MessageListState::new(),
            tool_registry: ToolCallRegistry::new(),
            content_cache: Arc::new(RwLock::new(ContentCache::new())),
        }
    }

    /// Get access to the content cache for rendering
    pub fn content_cache(&self) -> Arc<RwLock<ContentCache>> {
        self.content_cache.clone()
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

        // Pre-calculate height for new messages (they'll likely be rendered soon)
        self.preload_message_height(&content);
    }

    /// Add multiple messages efficiently (for restored conversations)
    pub fn add_messages(&mut self, messages: Vec<MessageContent>) {
        let count = messages.len();
        for message in messages {
            // Handle tool call registration for assistant messages
            if let MessageContent::Assistant { blocks, .. } = &message {
                for block in blocks {
                    if let AssistantContent::ToolCall { tool_call } = block {
                        self.tool_registry.register_call(tool_call.clone());
                    }
                }
            }

            // Add the message and get its index
            let message_index = self.messages.len();
            self.messages.push(message.clone());

            // For tool messages, set the message index in the registry
            if let MessageContent::Tool { id, call, .. } = &message {
                self.tool_registry.register_call(call.clone());
                self.tool_registry.set_message_index(id, message_index);
            }

            // Don't preload heights for bulk additions (restored messages)
        }

        // Log summary after bulk loading
        if count > 0 {
            if let Ok(cache) = self.content_cache.read() {
                tracing::debug!(target: "view_model", "Loaded {} messages. Cache ready for lazy loading.", count);
                cache.log_summary();
            }
        }
    }

    /// Pre-calculate height for a message with common view modes and widths
    fn preload_message_height(&self, content: &MessageContent) {
        use crate::tui::widgets::content_renderer::{ContentRenderer, DefaultContentRenderer};

        let renderer = DefaultContentRenderer;
        let common_widths = [80, 120, 160]; // Common terminal widths
        let view_modes = [ViewMode::Compact, ViewMode::Detailed];

        for &width in &common_widths {
            for &mode in &view_modes {
                if let Ok(mut cache) = self.content_cache.write() {
                    // Pre-calculate height for common configurations
                    cache.get_or_parse_height(content, mode, width, |msg, view_mode, w| {
                        renderer.calculate_height(msg, view_mode, w)
                    });
                }
            }
        }
    }

    /// Clear the content cache (e.g., on major UI changes)
    pub fn clear_content_cache(&mut self) {
        if let Ok(mut cache) = self.content_cache.write() {
            cache.clear();
        }
    }

    /// Invalidate cache for a specific message (when message content changes)
    pub fn invalidate_message_cache(&mut self, message_id: &str) {
        if let Ok(mut cache) = self.content_cache.write() {
            cache.invalidate_message(message_id);
        }
    }

    /// Get content cache statistics for debugging
    pub fn cache_stats(&self) -> (usize, usize, f64) {
        if let Ok(cache) = self.content_cache.read() {
            cache.stats()
        } else {
            (0, 0, 0.0)
        }
    }

    /// Preload heights for messages near the visible range
    pub fn preload_near_visible(
        &mut self,
        visible_range: &crate::tui::widgets::message_list::VisibleRange,
        width: u16,
    ) {
        use crate::tui::widgets::content_renderer::{ContentRenderer, DefaultContentRenderer};

        let renderer = DefaultContentRenderer;
        let buffer_size = 10; // Preload 10 messages above and below

        let start = visible_range.first_index.saturating_sub(buffer_size);
        let end = (visible_range.last_index + buffer_size).min(self.messages.len() - 1);

        for idx in start..=end {
            if let Some(message) = self.messages.get(idx) {
                let mode = self.message_list_state.view_prefs.determine_mode(message);

                if let Ok(mut cache) = self.content_cache.write() {
                    // Ensure height is cached
                    cache.get_or_parse_height(message, mode, width, |msg, view_mode, w| {
                        renderer.calculate_height(msg, view_mode, w)
                    });
                }
            }
        }
    }

    /// Log cache performance summary
    pub fn log_cache_performance(&self) {
        if let Ok(cache) = self.content_cache.read() {
            cache.log_summary();
        }
    }
}
