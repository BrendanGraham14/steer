//! MessageEventProcessor - handles message-related events.
//!
//! Processes events that add, update, or modify messages in the conversation,
//! including streaming message parts and message restoration.

use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::ChatItemData;
use async_trait::async_trait;
use conductor_core::app::AppEvent;
use conductor_core::app::conversation::AssistantContent;

/// Processor for message-related events
pub struct MessageEventProcessor;

impl MessageEventProcessor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
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
                | AppEvent::ActiveMessageIdChanged { .. }
        )
    }

    async fn process(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> ProcessingResult {
        match event {
            AppEvent::MessageAdded { message, .. } => {
                self.handle_message_added(message, ctx);
                ProcessingResult::Handled
            }
            AppEvent::MessageUpdated { id, content } => {
                // Find the message in the chat store
                let mut found = false;
                for item in ctx.chat_store.iter_mut() {
                    if let ChatItemData::Message(message) = &mut item.data {
                        if message.id() == id {
                            if let conductor_core::app::conversation::MessageData::Assistant {
                                content: blocks,
                                ..
                            } = &mut message.data
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
                    if let ChatItemData::Message(message) = &mut item.data {
                        if message.id() == id {
                            if let conductor_core::app::conversation::MessageData::Assistant {
                                content: blocks,
                                ..
                            } = &mut message.data
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
            AppEvent::ActiveMessageIdChanged { message_id } => {
                tracing::debug!(
                    target: "tui.message_event",
                    "ActiveMessageIdChanged: {:?}",
                    message_id
                );

                ctx.chat_store.set_active_message_id(message_id);
                *ctx.messages_updated = true;

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
        if let conductor_core::app::MessageData::Assistant { content, .. } = &message.data {
            tracing::debug!(
                target: "tui.message_event",
                "Processing Assistant message id={}",
                message.id
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
                }
            }
        }

        ctx.chat_store.add_message(message);
        *ctx.messages_updated = true;
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
    use crate::tui::widgets::ChatListState;
    use async_trait::async_trait;
    use conductor_core::app::AppCommand;
    use conductor_core::app::conversation::{AssistantContent, Message, MessageData};
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

    #[tokio::test]
    async fn test_assistant_message_updates_placeholder_tool_params() {
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
        let placeholder_msg = Message {
            data: MessageData::Tool {
                tool_use_id: tool_id.clone(),
                result: conductor_tools::ToolResult::External(
                    conductor_tools::result::ExternalResult {
                        tool_name: "unknown".to_string(),
                        payload: "Pending...".to_string(),
                    },
                ),
            },
            id: "tool_msg_id".to_string(),
            timestamp: chrono::Utc::now().timestamp() as u64,
            parent_message_id: None,
        };
        ctx.chat_store.add_message(placeholder_msg);
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

        let assistant_message = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall { tool_call }],
            },
            id: "msg_123".to_string(),
            timestamp: 1234567890,
            parent_message_id: None,
        };

        let mut in_flight_operations = std::collections::HashSet::new();
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
            in_flight_operations: &mut in_flight_operations,
        };

        // Process the Assistant message
        let result = processor
            .process(
                conductor_core::app::AppEvent::MessageAdded {
                    message: assistant_message,
                    model: conductor_core::api::Model::Claude3_5Sonnet20241022,
                },
                &mut ctx,
            )
            .await;

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
