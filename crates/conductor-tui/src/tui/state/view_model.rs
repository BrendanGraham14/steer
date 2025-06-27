//! MessageViewModel – migrated to use ChatStore only.
//!
//! This holds the canonical `ChatStore` (data) and the per-view `ChatListState`.
//! All message handling now goes through ChatStore directly.

use super::chat_store::ChatStore;
use super::content_cache::ContentCache;
use super::tool_registry::ToolCallRegistry;
use crate::tui::model::MessageRow;
use crate::tui::widgets::chat_list::ChatListState;
use conductor_core::app::conversation::{AssistantContent, Message};
use std::sync::{Arc, RwLock};

#[derive(Debug)]
pub struct MessageViewModel {
    /// New chat store for ChatItems
    pub chat_store: ChatStore,
    /// UI state for the chat list widget
    pub chat_list_state: ChatListState,
    /// Centralized tool call lifecycle tracking
    pub tool_registry: ToolCallRegistry,
    /// Content rendering cache for performance
    content_cache: Arc<RwLock<ContentCache>>,
    /// Currently active thread ID (None until first message)
    pub current_thread: Option<uuid::Uuid>,
}

impl Default for MessageViewModel {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageViewModel {
    pub fn new() -> Self {
        Self {
            chat_store: ChatStore::new(),
            chat_list_state: ChatListState::new(),
            tool_registry: ToolCallRegistry::new(),
            content_cache: Arc::new(RwLock::new(ContentCache::new())),
            current_thread: None, // No thread until first message
        }
    }

    /// Get access to the content cache for rendering
    pub fn content_cache(&self) -> Arc<RwLock<ContentCache>> {
        self.content_cache.clone()
    }

    /// Add a message to the view model, handling tool registry coordination automatically
    pub fn add_message(&mut self, message: Message) {
        // If this is the first message with a real thread, update current thread
        if self.current_thread.is_none() {
            self.current_thread = Some(*message.thread_id());
        }

        // Handle tool call registration for assistant messages
        if let Message::Assistant { content, .. } = &message {
            for block in content {
                if let AssistantContent::ToolCall { tool_call } = block {
                    self.tool_registry.register_call(tool_call.clone());
                }
            }
        }

        // Add the message and get its index
        let message_index = self.chat_store.add_message(message.clone());

        // For tool messages, set the message index in the registry
        if let Message::Tool { tool_use_id, .. } = &message {
            // Get the tool call from registry or create a placeholder
            let tool_call = self.tool_registry.get_tool_call(tool_use_id).cloned()
                .unwrap_or_else(|| conductor_tools::schema::ToolCall {
                    id: tool_use_id.clone(),
                    name: "unknown".to_string(),
                    parameters: serde_json::Value::Null,
                });
            self.tool_registry.register_call(tool_call);
            self.tool_registry.set_message_index(tool_use_id, message_index);
        }
    }

    /// Add multiple messages efficiently (for restored conversations)
    pub fn add_messages(&mut self, messages: Vec<Message>) {
        let count = messages.len();

        // If we have messages and no current thread set, use the thread from the first message
        if !messages.is_empty() && self.current_thread.is_none() {
            let thread_id = *messages[0].thread_id();
            self.current_thread = Some(thread_id);
        }

        for message in messages {
            // Handle tool call registration for assistant messages
            if let Message::Assistant { content, .. } = &message {
                for block in content {
                    if let AssistantContent::ToolCall { tool_call } = block {
                        self.tool_registry.register_call(tool_call.clone());
                    }
                }
            }

            // Add the message
            let message_index = self.chat_store.add_message(message.clone());

            // For tool messages, set the message index in the registry
            if let Message::Tool { tool_use_id, .. } = &message {
                let tool_call = self.tool_registry.get_tool_call(tool_use_id).cloned()
                    .unwrap_or_else(|| conductor_tools::schema::ToolCall {
                        id: tool_use_id.clone(),
                        name: "unknown".to_string(),
                        parameters: serde_json::Value::Null,
                    });
                self.tool_registry.register_call(tool_call);
                self.tool_registry.set_message_index(tool_use_id, message_index);
            }
        }

        // Log summary after bulk loading
        if count > 0 {
            tracing::debug!(target: "view_model", "Loaded {} messages.", count);
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

    /// Set the current thread ID
    pub fn set_thread(&mut self, thread_id: uuid::Uuid) {
        self.current_thread = Some(thread_id);
    }

    /// Get messages for the current thread only
    pub fn get_current_thread_messages(&self) -> Vec<&MessageRow> {
        self.chat_store.messages()
    }

    /// Get user messages in the current thread (for edit history)
    pub fn get_user_messages_in_thread(&self) -> Vec<(usize, &MessageRow)> {
        self.chat_store.user_messages()
    }


    /// Log cache performance summary
    pub fn log_cache_performance(&self) {
        if let Ok(cache) = self.content_cache.read() {
            cache.log_summary();
        }
    }
}