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
                | AppEvent::RestoredMessage { .. }
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
            AppEvent::RestoredMessage { message, model: _ } => {
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
                tracing::debug!(target: "tui.message", "MessagePart: {}", id);
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
        if let crate::app::Message::Assistant { ref content, .. } = message {
            for block in content {
                if let AssistantContent::ToolCall { tool_call } = block {
                    ctx.tool_registry.register_call(tool_call.clone());
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