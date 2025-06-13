//! ToolEventProcessor - handles tool call lifecycle events.
//!
//! Manages tool execution state, approval requests, completion, and failure events.

use crate::app::AppEvent;
use crate::app::conversation::ToolResult;
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::widgets::message_list::MessageContent;

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
                *ctx.spinner_state = 0;
                *ctx.progress_message = Some(format!("Executing tool: {}", name));

                let idx = ctx.get_or_create_tool_index(&id, Some(name.clone()));

                if let MessageContent::Tool { call, .. } = &mut ctx.messages[idx] {
                    call.name = name.clone();
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
                ctx.tool_registry.complete_execution(&id, ToolResult::Success {
                    output: result,
                });

                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            AppEvent::ToolCallFailed {
                name: _,
                error,
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
                let approval_info = tools::schema::ToolCall {
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