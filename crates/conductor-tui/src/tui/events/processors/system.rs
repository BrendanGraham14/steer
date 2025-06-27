//! SystemEventProcessor - handles system and configuration events.
//!
//! Processes events related to model changes, command responses, and other
//! system-level state changes.

use conductor_core::app::AppEvent;
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::{ChatItem, generate_row_id};

/// Processor for system-level events
pub struct SystemEventProcessor;

impl SystemEventProcessor {
    pub fn new() -> Self {
        Self
    }

    /// Handle command response by adding it to the chat store
    fn handle_command_response(
        &self,
        command: conductor_core::app::conversation::AppCommandType,
        response: conductor_core::app::conversation::CommandResponse,
        ctx: &mut ProcessingContext,
    ) {
        let chat_item = ChatItem::CmdResponse {
            id: generate_row_id(),
            cmd: command,
            resp: response,
            ts: time::OffsetDateTime::now_utc(),
        };
        ctx.chat_store.push(chat_item);
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
                self.handle_command_response(command, response, ctx);
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
