//! ProcessingStateProcessor - handles processing state events.
//!
//! Manages the overall processing state of the TUI, including thinking/processing
//! indicators, spinner state, and progress messages.

use crate::app::{
    AppEvent,
    conversation::{AppCommandType, UserContent},
};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};

/// Processor for events that affect the overall processing state
pub struct ProcessingStateProcessor;

impl ProcessingStateProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl EventProcessor for ProcessingStateProcessor {
    fn priority(&self) -> usize {
        10 // High priority - state changes should happen early
    }

    fn can_handle(&self, event: &AppEvent) -> bool {
        matches!(
            event,
            AppEvent::ThinkingStarted
                | AppEvent::ThinkingCompleted
                | AppEvent::Error { .. }
                | AppEvent::OperationCancelled { .. }
        )
    }

    fn process(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> ProcessingResult {
        match event {
            AppEvent::ThinkingStarted => {
                *ctx.is_processing = true;
                *ctx.spinner_state = 0;
                ProcessingResult::Handled
            }
            AppEvent::ThinkingCompleted | AppEvent::Error { .. } => {
                *ctx.is_processing = false;
                *ctx.progress_message = None;
                ProcessingResult::Handled
            }
            AppEvent::OperationCancelled { info } => {
                *ctx.is_processing = false;
                *ctx.progress_message = None;
                *ctx.current_tool_approval = None;

                // Add cancellation message to the UI
                let display_id = format!("cancellation_{}", chrono::Utc::now().timestamp_millis());
                let cancellation_message =
                    crate::tui::widgets::message_list::MessageContent::User {
                        id: display_id,
                        blocks: vec![UserContent::AppCommand {
                            command: AppCommandType::Cancel,
                            response: Some(crate::app::conversation::CommandResponse::Text(
                                format!("Operation cancelled: {}", info),
                            )),
                        }],
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    };
                ctx.messages.push(cancellation_message);
                *ctx.messages_updated = true;

                ProcessingResult::Handled
            }
            _ => ProcessingResult::NotHandled,
        }
    }

    fn name(&self) -> &'static str {
        "ProcessingStateProcessor"
    }
}

impl Default for ProcessingStateProcessor {
    fn default() -> Self {
        Self::new()
    }
}
