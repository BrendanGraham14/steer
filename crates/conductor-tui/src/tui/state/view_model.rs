//! MessageViewModel – migrated to use ChatStore only.
//!
//! This holds the canonical `ChatStore` (data) and the per-view `ChatListState`.
//! All message handling now goes through ChatStore directly.

use super::chat_store::ChatStore;
use super::tool_registry::ToolCallRegistry;
use crate::tui::widgets::chat_list::ChatListState;
use conductor_core::app::conversation::{AssistantContent, Message};

#[derive(Debug)]
pub struct MessageViewModel {
    /// New chat store for ChatItems
    pub chat_store: ChatStore,
    /// UI state for the chat list widget
    pub chat_list_state: ChatListState,
    /// Centralized tool call lifecycle tracking
    pub tool_registry: ToolCallRegistry,
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
        }
    }

    /// Add a message to the view model, handling tool registry coordination automatically
    pub fn add_message(&mut self, message: Message) {
        // Handle tool call registration for assistant messages
        if let Message::Assistant { content, .. } = &message {
            for block in content {
                if let AssistantContent::ToolCall { tool_call } = block {
                    self.tool_registry.register_call(tool_call.clone());
                }
            }
        }

        // Add the message
        self.chat_store.add_message(message.clone());

        // For tool messages, set the message index in the registry
        if let Message::Tool { tool_use_id, .. } = &message {
            // Get the tool call from registry or create a placeholder
            let tool_call = self
                .tool_registry
                .get_tool_call(tool_use_id)
                .cloned()
                .unwrap_or_else(|| conductor_tools::schema::ToolCall {
                    id: tool_use_id.clone(),
                    name: "unknown".to_string(),
                    parameters: serde_json::Value::Null,
                });
            self.tool_registry.register_call(tool_call);
        }
    }

    /// Add multiple messages efficiently (for restored conversations)
    pub fn add_messages(&mut self, messages: Vec<Message>) {
        let count = messages.len();

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
            self.chat_store.add_message(message.clone());

            // For tool messages, set the message index in the registry
            if let Message::Tool { tool_use_id, .. } = &message {
                let tool_call = self
                    .tool_registry
                    .get_tool_call(tool_use_id)
                    .cloned()
                    .unwrap_or_else(|| conductor_tools::schema::ToolCall {
                        id: tool_use_id.clone(),
                        name: "unknown".to_string(),
                        parameters: serde_json::Value::Null,
                    });
                self.tool_registry.register_call(tool_call);
            }
        }

        // Log summary after bulk loading
        if count > 0 {
            tracing::debug!(target: "view_model", "Loaded {} messages.", count);
        }
    }
}
