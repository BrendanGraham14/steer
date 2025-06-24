//! ToolEventProcessor - handles tool call lifecycle events.
//!
//! Manages tool execution state, approval requests, completion, and failure events.

use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::widgets::message_list::MessageContent;
use conductor_core::app::AppEvent;
use conductor_core::app::conversation::ToolResult;

/// Processor for tool-related events
pub struct ToolEventProcessor;

impl ToolEventProcessor {
    pub fn new() -> Self {
        Self
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

                let idx = ctx.get_or_create_tool_index(&id, Some(name.clone()));

                // The placeholder might have been created with null params if the registry
                // didn't have the ToolCall yet. Check again and update if needed.
                if let Some(real_call) = ctx.tool_registry.get_tool_call(&id) {
                    if let MessageContent::Tool { call, .. } = &mut ctx.messages[idx] {
                        if call.parameters.is_null() && !real_call.parameters.is_null() {
                            tracing::debug!(
                                target: "tui.tool_event",
                                "Updating Tool message {} with real parameters from registry",
                                id
                            );
                            *call = real_call.clone();
                        }
                    }
                } else {
                    tracing::warn!(
                        target: "tui.tool_event",
                        "No ToolCall found in registry for id={} at ToolCallStarted",
                        id
                    );
                    if let MessageContent::Tool { call, .. } = &mut ctx.messages[idx] {
                        call.name = name.clone();
                    }
                }

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

                let idx = ctx.get_or_create_tool_index(&id, None);

                if let MessageContent::Tool {
                    result: existing_result,
                    ..
                } = &mut ctx.messages[idx]
                {
                    *existing_result = Some(ToolResult::Success {
                        output: result.clone(),
                    });
                }

                // Complete the tool execution in registry
                ctx.tool_registry
                    .complete_execution(&id, ToolResult::Success { output: result });

                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            AppEvent::ToolCallFailed {
                name: _, error, id, ..
            } => {
                *ctx.progress_message = None;

                let idx = ctx.get_or_create_tool_index(&id, None);

                if let MessageContent::Tool {
                    result: existing_result,
                    ..
                } = &mut ctx.messages[idx]
                {
                    *existing_result = Some(ToolResult::Error {
                        error: error.clone(),
                    });
                }

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
    use crate::tui::state::{MessageStore, ToolCallRegistry};
    use crate::tui::widgets::message_list::{MessageContent, MessageListState};
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
        MessageStore,
        MessageListState,
        ToolCallRegistry,
        Arc<dyn AppCommandSink>,
        bool,
        Option<String>,
        usize,
        Option<ToolCall>,
        conductor_core::api::Model,
        bool,
    ) {
        let messages = MessageStore::new();
        let message_list_state = MessageListState::new();
        let tool_registry = ToolCallRegistry::new();
        let command_sink = Arc::new(MockCommandSink) as Arc<dyn AppCommandSink>;
        let is_processing = false;
        let progress_message = None;
        let spinner_state = 0;
        let current_tool_approval = None;
        let current_model = conductor_core::api::Model::Claude3_5Sonnet20241022;
        let messages_updated = false;

        (
            messages,
            message_list_state,
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
            mut messages,
            mut message_list_state,
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
        };

        let model = current_model;

        {
            let mut ctx = ProcessingContext {
                messages: &mut messages,
                message_list_state: &mut message_list_state,
                tool_registry: &mut tool_registry,
                command_sink: &command_sink,
                is_processing: &mut is_processing,
                progress_message: &mut progress_message,
                spinner_state: &mut spinner_state,
                current_tool_approval: &mut current_tool_approval,
                current_model: &mut current_model,
                messages_updated: &mut messages_updated,
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
                messages: &mut messages,
                message_list_state: &mut message_list_state,
                tool_registry: &mut tool_registry,
                command_sink: &command_sink,
                is_processing: &mut is_processing,
                progress_message: &mut progress_message,
                spinner_state: &mut spinner_state,
                current_tool_approval: &mut current_tool_approval,
                current_model: &mut current_model,
                messages_updated: &mut messages_updated,
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

        // Assert the placeholder kept the parameters
        let idx = tool_registry
            .get_message_index("id123")
            .expect("placeholder should exist");
        if let MessageContent::Tool { call, .. } = &messages[idx] {
            assert_eq!(call.parameters, full_call.parameters);
            assert_eq!(call.name, "view");
            assert_eq!(call.id, "id123");
        } else {
            panic!("Expected Tool message at index {}", idx);
        }
    }
}
