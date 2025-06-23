//! MessageEventProcessor - handles message-related events.
//!
//! Processes events that add, update, or modify messages in the conversation,
//! including streaming message parts and message restoration.

use crate::app::AppEvent;
use crate::app::conversation::AssistantContent;
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::widgets::message_list::MessageContent;

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
                if let Some(msg) = ctx.messages.iter_mut().find(|m| m.id() == id) {
                    match msg {
                        MessageContent::Assistant { blocks, .. } => {
                            blocks.clear();
                            blocks.push(AssistantContent::Text { text: content });
                            *ctx.messages_updated = true;
                        }
                        _ => {
                            tracing::warn!(target: "tui.message", "MessageUpdated for non-assistant message: {}", id);
                        }
                    }
                } else {
                    tracing::warn!(target: "tui.message", "MessageUpdated for unknown ID: {}", id);
                }
                ProcessingResult::Handled
            }
            AppEvent::MessagePart { id, delta } => {
                // For streaming messages, append to existing text blocks
                if let Some(msg) = ctx.messages.iter_mut().find(|m| m.id() == id) {
                    match msg {
                        MessageContent::Assistant { blocks, .. } => {
                            if let Some(AssistantContent::Text { text }) = blocks.last_mut() {
                                text.push_str(&delta);
                                *ctx.messages_updated = true;
                            } else {
                                blocks.push(AssistantContent::Text { text: delta });
                                *ctx.messages_updated = true;
                            }
                        }
                        _ => {
                            tracing::warn!(target: "tui.message", "MessagePart for non-assistant message: {}", id);
                        }
                    }
                } else {
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
    fn handle_message_added(&self, message: crate::app::Message, ctx: &mut ProcessingContext) {
        // First, extract tool calls from Assistant messages to register them
        if let crate::app::Message::Assistant { ref content, ref id, .. } = message {
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
                    
                    // already handled above
                    // Update registry entry with full parameters too
                    ctx.tool_registry.upsert_call(tool_call.clone());


                    // If we already created a placeholder Tool message for this id (e.g. via
                    // ToolCallStarted that arrived earlier) then update the placeholder with the
                    // real parameters (and name) so that formatters can successfully parse them
                    if let Some(idx) = ctx.tool_registry.get_message_index(&tool_call.id) {
                        if let MessageContent::Tool { call, .. } = &mut ctx.messages[idx] {
                            *call = tool_call.clone();
                        }
                    }
                }
            }
        }

        let content = ctx.convert_message(message);

        match &content {
            MessageContent::Tool {
                id, call, result, ..
            } => {
                let idx = ctx.get_or_create_tool_index(id, Some(call.name.clone()));

                if let MessageContent::Tool {
                    call: existing_call,
                    result: existing_result,
                    ..
                } = &mut ctx.messages[idx]
                {
                    *existing_call = call.clone();
                    if existing_result.is_none() {
                        *existing_result = result.clone();
                    }
                }

                *ctx.messages_updated = true;
            }
            _ => {
                ctx.messages.push(content);
                *ctx.messages_updated = true;
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
    use crate::app::conversation::{AssistantContent, Message};
    use crate::tui::events::processor::ProcessingContext;
    use crate::tui::state::{MessageStore, ToolCallRegistry};
    use crate::tui::widgets::message_list::{MessageContent, MessageListState};
    use serde_json::json;
    use tokio::sync::mpsc;
    use tools::schema::ToolCall;

    fn create_test_context() -> (
        MessageStore,
        MessageListState,
        ToolCallRegistry,
        mpsc::Sender<crate::app::command::AppCommand>,
        bool,
        Option<String>,
        usize,
        Option<ToolCall>,
        crate::api::Model,
        bool,
    ) {
        let messages = MessageStore::new();
        let message_list_state = MessageListState::new();
        let tool_registry = ToolCallRegistry::new();
        let (cmd_tx, _) = mpsc::channel(1);
        let is_processing = false;
        let progress_message = None;
        let spinner_state = 0;
        let current_tool_approval = None;
        let current_model = crate::api::Model::Claude3_5Sonnet20241022;
        let messages_updated = false;

        (
            messages,
            message_list_state,
            tool_registry,
            cmd_tx,
            is_processing,
            progress_message,
            spinner_state,
            current_tool_approval,
            current_model,
            messages_updated,
        )
    }

    #[test]
    fn test_assistant_message_updates_placeholder_tool_params() {
        let mut processor = MessageEventProcessor::new();
        let (
            mut messages,
            mut message_list_state,
            mut tool_registry,
            cmd_tx,
            mut is_processing,
            mut progress_message,
            mut spinner_state,
            mut current_tool_approval,
            mut current_model,
            mut messages_updated,
        ) = create_test_context();

        // First, create a placeholder Tool message (simulating what happens during ToolCallStarted)
        let tool_id = "test_tool_123".to_string();
        let placeholder_call = ToolCall {
            id: tool_id.clone(),
            name: "unknown".to_string(),
            parameters: serde_json::Value::Null, // This is the problem - null params
        };

        messages.push(MessageContent::Tool {
            id: tool_id.clone(),
            call: placeholder_call.clone(),
            result: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        });
        let tool_idx = messages.len() - 1;
        tool_registry.set_message_index(&tool_id, tool_idx);
        tool_registry.register_call(placeholder_call);

        // Verify the placeholder has null params
        if let MessageContent::Tool { call, .. } = messages.get(tool_idx).unwrap() {
            assert_eq!(call.parameters, serde_json::Value::Null);
            assert_eq!(call.name, "unknown");
        }

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
        };

        let mut ctx = ProcessingContext {
            messages: &mut messages,
            message_list_state: &mut message_list_state,
            tool_registry: &mut tool_registry,
            command_tx: &cmd_tx,
            is_processing: &mut is_processing,
            progress_message: &mut progress_message,
            spinner_state: &mut spinner_state,
            current_tool_approval: &mut current_tool_approval,
            current_model: &mut current_model,
            messages_updated: &mut messages_updated,
        };

        // Process the Assistant message
        let result = processor.process(
            crate::app::AppEvent::MessageAdded {
                message: assistant_message,
                model: crate::api::Model::Claude3_5Sonnet20241022,
            },
            &mut ctx,
        );

        assert!(matches!(result, ProcessingResult::Handled));

        // Verify the placeholder Tool message was updated with real params
        if let MessageContent::Tool { call, .. } = messages.get(tool_idx).unwrap() {
            assert_eq!(call.parameters, real_params);
            assert_eq!(call.name, "view");
            assert_eq!(call.id, tool_id);
        } else {
            panic!("Expected Tool message at index {}", tool_idx);
        }
    }
}