//! MessageEventProcessor - handles message-related events.
//!
//! Processes events that add, update, or modify messages in the conversation,
//! including streaming message parts and message restoration.

use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::ChatItemData;
use async_trait::async_trait;
use steer_grpc::client_api::{
    AssistantContent, ClientEvent, Message, MessageData, MessageId, ThoughtContent, ToolCall,
    ToolCallDelta,
};

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

    fn can_handle(&self, event: &ClientEvent) -> bool {
        matches!(
            event,
            ClientEvent::MessageAdded { .. }
                | ClientEvent::MessageUpdated { .. }
                | ClientEvent::MessageDelta { .. }
                | ClientEvent::ThinkingDelta { .. }
                | ClientEvent::ToolCallDelta { .. }
        )
    }

    async fn process(
        &mut self,
        event: ClientEvent,
        ctx: &mut ProcessingContext,
    ) -> ProcessingResult {
        match event {
            ClientEvent::MessageAdded { message, .. } => {
                self.handle_message_added(message, ctx);
                ProcessingResult::Handled
            }
            ClientEvent::MessageUpdated { message } => {
                self.handle_message_updated(message, ctx);
                ProcessingResult::Handled
            }
            ClientEvent::MessageDelta { id, delta } => {
                self.handle_message_delta(&id, delta, ctx);
                ProcessingResult::Handled
            }
            ClientEvent::ThinkingDelta {
                message_id, delta, ..
            } => {
                self.handle_thinking_delta(&message_id, delta, ctx);
                ProcessingResult::Handled
            }
            ClientEvent::ToolCallDelta {
                message_id,
                tool_call_id,
                delta,
                ..
            } => {
                self.handle_tool_call_delta(&message_id, tool_call_id.as_str(), delta, ctx);
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
    fn handle_message_added(&self, message: Message, ctx: &mut ProcessingContext) {
        if let MessageData::Assistant { content, .. } = &message.data {
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

        if let Some(item) = ctx.chat_store.get_mut_by_id(&message.id.clone()) {
            item.data = ChatItemData::Message(message);
        } else {
            ctx.chat_store.add_message(message);
        }
        *ctx.messages_updated = true;
    }

    fn handle_message_updated(&self, message: Message, ctx: &mut ProcessingContext) {
        self.handle_message_added(message, ctx);
    }

    fn handle_message_delta(&self, id: &MessageId, delta: String, ctx: &mut ProcessingContext) {
        if !self.append_text_delta(id, &delta, ctx) {
            self.insert_placeholder_message(id, ctx);
            if !self.append_text_delta(id, &delta, ctx) {
                tracing::warn!(target: "tui.message", "MessageDelta received for unknown ID: {}", id);
            }
        }
    }

    fn handle_thinking_delta(&self, id: &MessageId, delta: String, ctx: &mut ProcessingContext) {
        if !self.append_thinking_delta(id, &delta, ctx) {
            self.insert_placeholder_message(id, ctx);
            if !self.append_thinking_delta(id, &delta, ctx) {
                tracing::warn!(target: "tui.message", "ThinkingDelta received for unknown ID: {}", id);
            }
        }
    }

    fn handle_tool_call_delta(
        &self,
        id: &MessageId,
        tool_call_id: &str,
        delta: ToolCallDelta,
        ctx: &mut ProcessingContext,
    ) {
        if !self.apply_tool_call_delta(id, tool_call_id, &delta, ctx) {
            self.insert_placeholder_message(id, ctx);
            if !self.apply_tool_call_delta(id, tool_call_id, &delta, ctx) {
                tracing::warn!(target: "tui.message", "ToolCallDelta received for unknown ID: {}", id);
            }
        }
    }

    fn insert_placeholder_message(&self, id: &MessageId, ctx: &mut ProcessingContext) {
        let message = Message {
            data: MessageData::Assistant {
                content: Vec::new(),
            },
            timestamp: time::OffsetDateTime::now_utc().unix_timestamp() as u64,
            id: id.as_str().to_string(),
            parent_message_id: ctx.chat_store.active_message_id().cloned(),
        };
        ctx.chat_store.add_message(message);
        *ctx.messages_updated = true;
    }

    fn append_text_delta(&self, id: &MessageId, delta: &str, ctx: &mut ProcessingContext) -> bool {
        for item in ctx.chat_store.iter_mut() {
            if let ChatItemData::Message(message) = &mut item.data
                && message.id() == id.as_str()
            {
                if let MessageData::Assistant {
                    content: blocks, ..
                } = &mut message.data
                {
                    if let Some(AssistantContent::Text { text }) = blocks.last_mut() {
                        text.push_str(delta);
                    } else {
                        blocks.push(AssistantContent::Text {
                            text: delta.to_string(),
                        });
                    }
                    *ctx.messages_updated = true;
                    return true;
                }
                tracing::warn!(
                    target: "tui.message",
                    "TextDelta for non-assistant message: {}",
                    id
                );
                return true;
            }
        }
        false
    }

    fn append_thinking_delta(id: &MessageId, delta: &str, ctx: &mut ProcessingContext) -> bool {
        for item in ctx.chat_store.iter_mut() {
            if let ChatItemData::Message(message) = &mut item.data
                && message.id() == id.as_str()
            {
                if let MessageData::Assistant {
                    content: blocks, ..
                } = &mut message.data
                {
                    if let Some(AssistantContent::Thought {
                        thought: ThoughtContent::Simple { text },
                    }) = blocks.last_mut()
                    {
                        if let Some(AssistantContent::Text { text }) = blocks.last_mut() {
                            text.push_str(delta);
                        } else {
                            blocks.push(AssistantContent::Text {
                                text: delta.to_string(),
                            });
                        }
                        *ctx.messages_updated = true;
                        return true;
                    }
                    tracing::warn!(
                        target: "tui.message",
                        "MessageDelta for non-assistant message: {}",
                        id
                    );
                    return true;
                }
            }
        }
        false
    }

    fn append_thinking_delta(
        &self,
        id: &MessageId, delta: &str, ctx: &mut ProcessingContext) -> bool {
        for item in ctx.chat_store.iter_mut() {
            if let ChatItemData::Message(message) = &mut item.data {
                if message.id() == id.as_str() {
                    if let MessageData::Assistant {
                        content: blocks, ..
                    } = &mut message.data
                    {
                        match blocks.last_mut() {
                            Some(AssistantContent::Thought {
                                thought: ThoughtContent::Simple { text },
                            }) => {
                                text.push_str(delta);
                            }
                            _ => {
                                blocks.push(AssistantContent::Thought {
                                    thought: ThoughtContent::Simple {
                                        text: delta.to_string(),
                                    },
                                });
                            }
                        }
                        *ctx.messages_updated = true;
                        return true;
                    }
                    tracing::warn!(
                        target: "tui.message",
                        "ThinkingDelta for non-assistant message: {}",
                        id
                    );
                    return true;
                }
            }
        }
        false
    }

    fn apply_tool_call_delta(
        &self,
        id: &MessageId,
        tool_call_id: &str,
        delta: &ToolCallDelta,
        ctx: &mut ProcessingContext,
    ) -> bool {
        for item in ctx.chat_store.iter_mut() {
            if let ChatItemData::Message(message) = &mut item.data {
                if message.id() == id.as_str() {
                    if let MessageData::Assistant {
                        content: blocks, ..
                    } = &mut message.data
                    {
                        let tool_call = match blocks.iter_mut().find_map(|block| {
                            if let AssistantContent::ToolCall { tool_call } = block {
                                if tool_call.id == tool_call_id {
                                    return Some(tool_call);
                                }
                            }
                            None
                        }) {
                            Some(existing) => existing,
                            None => {
                                blocks.push(AssistantContent::ToolCall {
                                    tool_call: ToolCall {
                                        id: tool_call_id.to_string(),
                                        name: "unknown".to_string(),
                                        parameters: serde_json::Value::String(String::new()),
                                    },
                                });
                                match blocks.last_mut() {
                                    Some(AssistantContent::ToolCall { tool_call }) => tool_call,
                                    _ => return false,
                                }
                            }
                        };

                        match delta {
                            ToolCallDelta::Name(name) => {
                                tool_call.name = name.clone();
                            }
                            ToolCallDelta::ArgumentChunk(chunk) => {
                                match tool_call.parameters.as_str() {
                                    Some(existing) => {
                                        let mut updated = existing.to_string();
                                        updated.push_str(chunk);
                                        tool_call.parameters = serde_json::Value::String(updated);
                                    }
                                    None => {
                                        tool_call.parameters =
                                            serde_json::Value::String(chunk.clone());
                                    }
                                }
                            }
                        }
                        *ctx.messages_updated = true;
                        return true;
                    }
                    *ctx.messages_updated = true;
                    return true;
                }
                tracing::warn!(
                    target: "tui.message",
                    "ToolCallDelta for non-assistant message: {}",
                    id
                );
                return true;
            }
        }
        false
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

    use serde_json::json;

    use crate::tui::events::processor::PendingToolApproval;
    use steer_grpc::AgentClient;
    use steer_grpc::client_api::{AssistantContent, Message, MessageData, ModelId, ToolCall};

    struct TestContext {
        chat_store: ChatStore,
        chat_list_state: ChatListState,
        tool_registry: ToolCallRegistry,
        client: AgentClient,
        is_processing: bool,
        progress_message: Option<String>,
        spinner_state: usize,
        current_tool_approval: Option<PendingToolApproval>,
        current_model: ModelId,
        messages_updated: bool,
    }

    async fn create_test_context() -> TestContext {
        let chat_store = ChatStore::new();
        let chat_list_state = ChatListState::new();
        let tool_registry = ToolCallRegistry::new();
        let (client, _server_handle) = crate::tui::test_utils::local_client_and_server(None).await;
        let is_processing = false;
        let progress_message = None;
        let spinner_state = 0;
        let current_tool_approval = None;
        let current_model = steer_core::config::model::builtin::claude_3_5_sonnet_20241022();
        let messages_updated = false;

        TestContext {
            chat_store,
            chat_list_state,
            tool_registry,
            client,
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
        let mut ctx = create_test_context().await;

        // First, create a placeholder Tool message (simulating what happens during ToolStarted)
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
                result: steer_grpc::client_api::ToolResult::External(
                    steer_grpc::client_api::ExternalResult {
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
            client: &ctx.client,
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
                ClientEvent::MessageAdded {
                    message: assistant_message,
                    model: steer_core::config::model::builtin::claude_3_5_sonnet_20241022(),
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
