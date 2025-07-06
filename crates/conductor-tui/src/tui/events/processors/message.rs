//! MessageEventProcessor - handles message-related events.
//!
//! Processes events that add, update, or modify messages in the conversation,
//! including streaming message parts and message restoration.

use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::ChatItem;
use conductor_core::app::AppEvent;
use conductor_core::app::conversation::AssistantContent;

/// Processor for message-related events
pub struct MessageEventProcessor;

impl MessageEventProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl EventProcessor for MessageEventProcessor {
    fn priority(&self) -> usize {
        50 // Medium priority - after state changes but before tool events
    }

    fn can_handle(&self, event: &AppEvent) -> bool {
        matches!(
            event,
            AppEvent::MessageAdded { .. }
                | AppEvent::MessageUpdated { .. }
                | AppEvent::MessagePart { .. }
        )
    }

    fn process(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> ProcessingResult {
        match event {
            AppEvent::MessageAdded { message, .. } => {
                self.handle_message_added(message, ctx);
                ProcessingResult::Handled
            }
            AppEvent::MessageUpdated { id, content } => {
                // Find the message in the chat store
                let mut found = false;
                for item in ctx.chat_store.iter_mut() {
                    if let ChatItem::Message(row) = item {
                        if row.inner.id() == id {
                            if let conductor_core::app::conversation::Message::Assistant {
                                content: blocks,
                                ..
                            } = &mut row.inner
                            {
                                blocks.clear();
                                blocks.push(AssistantContent::Text { text: content });
                                *ctx.messages_updated = true;
                                found = true;
                                break;
                            } else {
                                tracing::warn!(target: "tui.message", "MessageUpdated for non-assistant message: {}", id);
                                break;
                            }
                        }
                    }
                }
                if !found {
                    tracing::warn!(target: "tui.message", "MessageUpdated for unknown ID: {}", id);
                }
                ProcessingResult::Handled
            }
            AppEvent::MessagePart { id, delta } => {
                // For streaming messages, append to existing text blocks
                let mut found = false;
                for item in ctx.chat_store.iter_mut() {
                    if let ChatItem::Message(row) = item {
                        if row.inner.id() == id {
                            if let conductor_core::app::conversation::Message::Assistant {
                                content: blocks,
                                ..
                            } = &mut row.inner
                            {
                                if let Some(AssistantContent::Text { text }) = blocks.last_mut() {
                                    text.push_str(&delta);
                                    *ctx.messages_updated = true;
                                } else {
                                    blocks.push(AssistantContent::Text { text: delta });
                                    *ctx.messages_updated = true;
                                }
                                found = true;
                                break;
                            } else {
                                tracing::warn!(target: "tui.message", "MessagePart for non-assistant message: {}", id);
                                break;
                            }
                        }
                    }
                }
                if !found {
                    tracing::warn!(target: "tui.message", "MessagePart received for unknown ID: {}", id);
                }
                ProcessingResult::Handled
            }
            _ => ProcessingResult::NotHandled,
        }
    }

    fn name(&self) -> &'static str {
        "MessageEventProcessor"
    }
}

impl MessageEventProcessor {
    fn handle_message_added(
        &self,
        message: conductor_core::app::Message,
        ctx: &mut ProcessingContext,
    ) {
        let old_thread_id = ctx.chat_store.current_thread();
        let new_message_thread_id = *message.thread_id();

        // First, extract tool calls from Assistant messages to register them
        if let conductor_core::app::Message::Assistant {
            ref content,
            ref id,
            ..
        } = message
        {
            tracing::debug!(
                target: "tui.message_event",
                "Processing Assistant message id={}",
                id
            );
            for block in content {
                if let AssistantContent::ToolCall { tool_call } = block {
                    tracing::debug!(
                        target: "tui.message_event",
                        "Found ToolCall in Assistant message: id={}, name={}, params={}",
                        tool_call.id, tool_call.name, tool_call.parameters
                    );

                    // Update registry entry with full parameters
                    ctx.tool_registry.upsert_call(tool_call.clone());

                    // If we already created a placeholder Tool message for this id,
                    // update it with the real parameters
                    if let Some(idx) = ctx.tool_registry.get_message_index(&tool_call.id) {
                        if let Some(ChatItem::Message(row)) = ctx.chat_store.get_mut(idx) {
                            if let conductor_core::app::conversation::Message::Tool {
                                tool_use_id,
                                ..
                            } = &mut row.inner
                            {
                                if tool_use_id == &tool_call.id {
                                    // Tool messages are already properly handled, we just needed to update the registry
                                }
                            }
                        }
                    }
                }
            }
        }

        // Add the message to the store
        ctx.chat_store.add_message(message);
        *ctx.messages_updated = true;

        // After adding, check if a thread switch occurred, and prune if so.
        if let Some(old_id) = old_thread_id {
            if old_id != new_message_thread_id {
                tracing::debug!(
                    target: "tui.message_event",
                    "Thread switch detected from {} to {}. Pruning message store.",
                    old_id, new_message_thread_id
                );
                ctx.chat_store.prune_to_thread(new_message_thread_id);
            }
        }
    }
}

impl Default for MessageEventProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::events::processor::ProcessingContext;
    use crate::tui::state::{ChatStore, ToolCallRegistry};
    use crate::tui::widgets::chat_list::ChatListState;
    use async_trait::async_trait;
    use conductor_core::app::AppCommand;
    use conductor_core::app::conversation::{AssistantContent, Message};
    use conductor_core::app::io::AppCommandSink;
    use conductor_core::error::Result;
    use conductor_tools::schema::ToolCall;
    use serde_json::json;
    use std::sync::Arc;

    // Mock command sink for tests
    struct MockCommandSink;

    #[async_trait]
    impl AppCommandSink for MockCommandSink {
        async fn send_command(&self, _command: AppCommand) -> Result<()> {
            Ok(())
        }
    }

    struct TestContext {
        chat_store: ChatStore,
        chat_list_state: ChatListState,
        tool_registry: ToolCallRegistry,
        command_sink: Arc<dyn AppCommandSink>,
        is_processing: bool,
        progress_message: Option<String>,
        spinner_state: usize,
        current_tool_approval: Option<ToolCall>,
        current_model: conductor_core::api::Model,
        messages_updated: bool,
    }

    fn create_test_context() -> TestContext {
        let chat_store = ChatStore::new();
        let chat_list_state = ChatListState::new();
        let tool_registry = ToolCallRegistry::new();
        let command_sink = Arc::new(MockCommandSink) as Arc<dyn AppCommandSink>;
        let is_processing = false;
        let progress_message = None;
        let spinner_state = 0;
        let current_tool_approval = None;
        let current_model = conductor_core::api::Model::Claude3_5Sonnet20241022;
        let messages_updated = false;

        TestContext {
            chat_store,
            chat_list_state,
            tool_registry,
            command_sink,
            is_processing,
            progress_message,
            spinner_state,
            current_tool_approval,
            current_model,
            messages_updated,
        }
    }

    #[test]
    fn test_assistant_message_updates_placeholder_tool_params() {
        let mut processor = MessageEventProcessor::new();
        let mut ctx = create_test_context();

        // First, create a placeholder Tool message (simulating what happens during ToolCallStarted)
        let tool_id = "test_tool_123".to_string();
        let placeholder_call = ToolCall {
            id: tool_id.clone(),
            name: "unknown".to_string(),
            parameters: serde_json::Value::Null, // This is the problem - null params
        };

        // Create a placeholder Tool message
        let placeholder_msg = Message::Tool {
            id: "tool_msg_id".to_string(),
            tool_use_id: tool_id.clone(),
            result: conductor_tools::ToolResult::External(
                conductor_tools::result::ExternalResult {
                    tool_name: "unknown".to_string(),
                    payload: "Pending...".to_string(),
                },
            ),
            timestamp: chrono::Utc::now().timestamp() as u64,
            thread_id: uuid::Uuid::new_v4(),
            parent_message_id: None,
        };
        let tool_idx = ctx.chat_store.add_message(placeholder_msg);
        ctx.tool_registry.set_message_index(&tool_id, tool_idx);
        ctx.tool_registry.register_call(placeholder_call);

        // Verify the placeholder was created
        assert_eq!(ctx.chat_store.len(), 1);

        // Now process an Assistant message with the real ToolCall
        let real_params = json!({
            "file_path": "/test/file.rs",
            "offset": 10,
            "limit": 100
        });

        let tool_call = ToolCall {
            id: tool_id.clone(),
            name: "view".to_string(),
            parameters: real_params.clone(),
        };

        let assistant_message = Message::Assistant {
            id: "msg_123".to_string(),
            content: vec![AssistantContent::ToolCall { tool_call }],
            timestamp: 1234567890,
            thread_id: uuid::Uuid::new_v4(),
            parent_message_id: None,
        };

        let current_thread = uuid::Uuid::new_v4();
        let mut ctx = ProcessingContext {
            chat_store: &mut ctx.chat_store,
            chat_list_state: &mut ctx.chat_list_state,
            tool_registry: &mut ctx.tool_registry,
            command_sink: &ctx.command_sink,
            is_processing: &mut ctx.is_processing,
            progress_message: &mut ctx.progress_message,
            spinner_state: &mut ctx.spinner_state,
            current_tool_approval: &mut ctx.current_tool_approval,
            current_model: &mut ctx.current_model,
            messages_updated: &mut ctx.messages_updated,
            current_thread: Some(current_thread),
        };

        // Process the Assistant message
        let result = processor.process(
            conductor_core::app::AppEvent::MessageAdded {
                message: assistant_message,
                model: conductor_core::api::Model::Claude3_5Sonnet20241022,
            },
            &mut ctx,
        );

        assert!(matches!(result, ProcessingResult::Handled));

        // Verify the registry was updated with real params
        let stored_call = ctx
            .tool_registry
            .get_tool_call(&tool_id)
            .expect("Tool call should be in registry");
        assert_eq!(stored_call.parameters, real_params);
        assert_eq!(stored_call.name, "view");
        assert_eq!(stored_call.id, tool_id);
    }
}
