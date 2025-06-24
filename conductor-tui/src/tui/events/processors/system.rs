//! SystemEventProcessor - handles system and configuration events.
//!
//! Processes events related to model changes, command responses, and other
//! system-level state changes.

use crate::app::{AppEvent, conversation::UserContent};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::widgets::message_list::MessageContent;

/// Processor for system-level events
pub struct SystemEventProcessor;

impl SystemEventProcessor {
    pub fn new() -> Self {
        Self
    }

    /// Create a user command response message
    fn create_command_response(
        id: String,
        command: crate::app::conversation::AppCommandType,
        response: crate::app::conversation::CommandResponse,
    ) -> MessageContent {
        MessageContent::User {
            id,
            blocks: vec![UserContent::AppCommand {
                command,
                response: Some(response),
            }],
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

impl EventProcessor for SystemEventProcessor {
    fn priority(&self) -> usize {
        90 // Low priority - run after most other processors
    }

    fn can_handle(&self, event: &AppEvent) -> bool {
        matches!(
            event,
            AppEvent::ModelChanged { .. } | AppEvent::CommandResponse { .. }
        )
    }

    fn process(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> ProcessingResult {
        match event {
            AppEvent::ModelChanged { model } => {
                *ctx.current_model = model;
                ProcessingResult::Handled
            }
            AppEvent::CommandResponse {
                command,
                response,
                id: _,
            } => {
                let response_id = format!("cmd_resp_{}", chrono::Utc::now().timestamp_millis());
                let response_message =
                    Self::create_command_response(response_id, command, response);
                ctx.messages.push(response_message);
                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            _ => ProcessingResult::NotHandled,
        }
    }

    fn name(&self) -> &'static str {
        "SystemEventProcessor"
    }
}

impl Default for SystemEventProcessor {
    fn default() -> Self {
        Self::new()
    }
}
