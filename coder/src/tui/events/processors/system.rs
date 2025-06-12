//! SystemEventProcessor - handles system and configuration events.
//!
//! Processes events related to model changes, command responses, and other
//! system-level state changes.

use crate::app::AppEvent;
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::widgets::message_list::MessageContent;

/// Processor for system-level events
pub struct SystemEventProcessor;

impl SystemEventProcessor {
    pub fn new() -> Self {
        Self
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
                tracing::debug!(target: "tui.system", "Model changed to: {}", model);
                *ctx.current_model = model;
                ProcessingResult::Handled
            }
            AppEvent::CommandResponse { content, id: _ } => {
                let response_id = format!("cmd_resp_{}", chrono::Utc::now().timestamp_millis());
                let response_message = MessageContent::System {
                    id: response_id,
                    text: content,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                };
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