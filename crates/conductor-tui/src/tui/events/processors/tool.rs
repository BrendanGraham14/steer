//! ToolEventProcessor - handles tool call lifecycle events.
//!
//! Manages tool execution state, approval requests, completion, and failure events.

use crate::notifications::{NotificationConfig, NotificationSound, notify_with_sound};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::ChatItem;
use conductor_core::app::AppEvent;
use conductor_core::app::conversation::ToolResult;
use conductor_tools::error::ToolError;

/// Processor for tool-related events
pub struct ToolEventProcessor {
    notification_config: NotificationConfig,
}

impl ToolEventProcessor {
    pub fn new() -> Self {
        Self {
            notification_config: NotificationConfig::from_env(),
        }
    }
}

impl EventProcessor for ToolEventProcessor {
    fn priority(&self) -> usize {
        75 // After message events but before system events
    }

    fn can_handle(&self, event: &AppEvent) -> bool {
        matches!(
            event,
            AppEvent::ToolCallStarted { .. }
                | AppEvent::ToolCallCompleted { .. }
                | AppEvent::ToolCallFailed { .. }
                | AppEvent::RequestToolApproval { .. }
        )
    }

    fn process(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> ProcessingResult {
        match event {
            AppEvent::ToolCallStarted { name, id, .. } => {
                tracing::debug!(
                    target: "tui.tool_event",
                    "ToolCallStarted: id={}, name={}",
                    id, name
                );

                // Debug: dump registry state
                ctx.tool_registry.debug_dump("At ToolCallStarted");

                *ctx.spinner_state = 0;
                *ctx.progress_message = Some(format!("Executing tool: {}", name));

                // Get the tool call from the registry
                let tool_call = if let Some(call) = ctx.tool_registry.get_tool_call(&id) {
                    call.clone()
                } else {
                    // Create a placeholder tool call if not found
                    conductor_tools::schema::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        parameters: serde_json::Value::Null,
                    }
                };

                // Add a pending tool call item
                let pending = ChatItem::PendingToolCall {
                    id: crate::tui::model::generate_row_id(),
                    tool_call,
                    ts: time::OffsetDateTime::now_utc(),
                };

                let idx = ctx.chat_store.add_pending_tool(pending);
                ctx.tool_registry.set_message_index(&id, idx);

                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            AppEvent::ToolCallCompleted {
                name: _,
                result,
                id,
                ..
            } => {
                *ctx.progress_message = None;

                // Find and remove the pending tool call
                if let Some(idx) = ctx.tool_registry.get_message_index(&id) {
                    if let Some(ChatItem::PendingToolCall { .. }) = ctx.chat_store.get(idx) {
                        ctx.chat_store.remove(idx);
                    }
                }

                // Create a complete tool message
                let tool_msg = conductor_core::app::conversation::Message::Tool {
                    id: crate::tui::model::generate_row_id(),
                    tool_use_id: id.clone(),
                    result: result.clone(),
                    timestamp: chrono::Utc::now().timestamp() as u64,
                    thread_id: ctx.current_thread.unwrap_or(uuid::Uuid::new_v4()),
                    parent_message_id: None,
                };

                let _idx = ctx.chat_store.add_message(tool_msg);

                // Complete the tool execution in registry
                ctx.tool_registry.complete_execution(&id, result);

                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            AppEvent::ToolCallFailed {
                name, error, id, ..
            } => {
                *ctx.progress_message = None;

                // Find and remove the pending tool call
                if let Some(idx) = ctx.tool_registry.get_message_index(&id) {
                    if let Some(ChatItem::PendingToolCall { .. }) = ctx.chat_store.get(idx) {
                        ctx.chat_store.remove(idx);
                    }
                }

                // Create a complete tool message with error
                let tool_msg = conductor_core::app::conversation::Message::Tool {
                    id: crate::tui::model::generate_row_id(),
                    tool_use_id: id.clone(),
                    result: ToolResult::Error(ToolError::Execution {
                        tool_name: name.clone(),
                        message: error.clone(),
                    }),
                    timestamp: chrono::Utc::now().timestamp() as u64,
                    thread_id: ctx.current_thread.unwrap_or(uuid::Uuid::new_v4()),
                    parent_message_id: None,
                };

                let _idx = ctx.chat_store.add_message(tool_msg);

                // Complete the tool execution in registry with error
                ctx.tool_registry.fail_execution(&id, error);

                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            AppEvent::RequestToolApproval {
                name,
                parameters,
                id,
                ..
            } => {
                let approval_info = conductor_tools::schema::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    parameters: parameters.clone(),
                };

                *ctx.current_tool_approval = Some(approval_info);

                // Notify user about tool approval request
                notify_with_sound(
                    &self.notification_config,
                    NotificationSound::ToolApproval,
                    &format!("Tool approval needed: {}", name),
                );

                ProcessingResult::Handled
            }
            _ => ProcessingResult::NotHandled,
        }
    }

    fn name(&self) -> &'static str {
        "ToolEventProcessor"
    }
}

impl Default for ToolEventProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::events::processor::ProcessingContext;
    use crate::tui::events::processors::message::MessageEventProcessor;
    use crate::tui::state::{ChatStore, ToolCallRegistry};
    use crate::tui::widgets::chat_list::ChatListState;
    use anyhow::Result;
    use async_trait::async_trait;
    use conductor_core::app::AppCommand;
    use conductor_core::app::conversation::{AssistantContent, Message};
    use conductor_core::app::io::AppCommandSink;
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

    fn create_test_context() -> (
        ChatStore,
        ChatListState,
        ToolCallRegistry,
        Arc<dyn AppCommandSink>,
        bool,
        Option<String>,
        usize,
        Option<ToolCall>,
        conductor_core::api::Model,
        bool,
    ) {
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

        (
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
        )
    }

    #[test]
    fn test_toolcallstarted_after_assistant_keeps_params() {
        let mut tool_proc = ToolEventProcessor::new();
        let mut msg_proc = MessageEventProcessor::new();
        let (
            mut chat_store,
            mut chat_list_state,
            mut tool_registry,
            command_sink,
            mut is_processing,
            mut progress_message,
            mut spinner_state,
            mut current_tool_approval,
            mut current_model,
            mut messages_updated,
        ) = create_test_context();

        // Full tool call we expect to keep
        let full_call = ToolCall {
            id: "id123".to_string(),
            name: "view".to_string(),
            parameters: json!({"file_path": "/tmp/x", "offset": 1}),
        };

        // 1. Assistant message first - populates registry with full params
        let assistant = Message::Assistant {
            id: "a1".to_string(),
            content: vec![AssistantContent::ToolCall {
                tool_call: full_call.clone(),
            }],
            timestamp: 0,
            thread_id: uuid::Uuid::new_v4(),
            parent_message_id: None,
        };

        let model = current_model;

        {
            let mut ctx = ProcessingContext {
                chat_store: &mut chat_store,
                chat_list_state: &mut chat_list_state,
                tool_registry: &mut tool_registry,
                command_sink: &command_sink,
                is_processing: &mut is_processing,
                progress_message: &mut progress_message,
                spinner_state: &mut spinner_state,
                current_tool_approval: &mut current_tool_approval,
                current_model: &mut current_model,
                messages_updated: &mut messages_updated,
                current_thread: None,
            };
            msg_proc.process(
                conductor_core::app::AppEvent::MessageAdded {
                    message: assistant,
                    model,
                },
                &mut ctx,
            );
        }

        // 2. ToolCallStarted arrives afterwards
        {
            let mut ctx = ProcessingContext {
                chat_store: &mut chat_store,
                chat_list_state: &mut chat_list_state,
                tool_registry: &mut tool_registry,
                command_sink: &command_sink,
                is_processing: &mut is_processing,
                progress_message: &mut progress_message,
                spinner_state: &mut spinner_state,
                current_tool_approval: &mut current_tool_approval,
                current_model: &mut current_model,
                messages_updated: &mut messages_updated,
                current_thread: None,
            };
            tool_proc.process(
                conductor_core::app::AppEvent::ToolCallStarted {
                    name: "view".to_string(),
                    id: "id123".to_string(),
                    model,
                },
                &mut ctx,
            );
        }

        // Assert the tool was registered and stored properly
        let stored_call = tool_registry
            .get_tool_call("id123")
            .expect("tool call should exist");
        assert_eq!(stored_call.parameters, full_call.parameters);
        assert_eq!(stored_call.name, "view");
        assert_eq!(stored_call.id, "id123");
    }
}
