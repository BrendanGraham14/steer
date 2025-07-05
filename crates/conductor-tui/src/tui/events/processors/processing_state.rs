//! ProcessingStateProcessor - handles processing state events.
//!
//! Manages the overall processing state of the TUI, including thinking/processing
//! indicators, spinner state, progress messages, and completion notifications.

use crate::notifications::{NotificationConfig, NotificationSound, notify_with_sound};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use conductor_core::app::AppEvent;

/// Processor for events that affect the overall processing state
pub struct ProcessingStateProcessor {
    notification_config: NotificationConfig,
}

impl ProcessingStateProcessor {
    pub fn new() -> Self {
        Self {
            notification_config: NotificationConfig::from_env(),
        }
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
            AppEvent::ThinkingCompleted => {
                let was_processing = *ctx.is_processing;
                *ctx.is_processing = false;
                *ctx.progress_message = None;

                // Trigger notification if we were processing
                if was_processing {
                    notify_with_sound(
                        &self.notification_config,
                        NotificationSound::ProcessingComplete,
                        "Processing complete - waiting for input",
                    );
                }

                ProcessingResult::Handled
            }
            AppEvent::Error { .. } => {
                let was_processing = *ctx.is_processing;
                *ctx.is_processing = false;
                *ctx.progress_message = None;

                // Trigger error notification if we were processing
                if was_processing {
                    notify_with_sound(
                        &self.notification_config,
                        NotificationSound::Error,
                        "An error occurred during processing",
                    );
                }

                ProcessingResult::Handled
            }
            AppEvent::OperationCancelled { info } => {
                *ctx.is_processing = false;
                *ctx.progress_message = None;
                *ctx.current_tool_approval = None;

                // Add cancellation message to the UI
                let chat_item = crate::tui::model::ChatItem::SystemNotice {
                    id: crate::tui::model::generate_row_id(),
                    level: crate::tui::model::NoticeLevel::Info,
                    text: format!("Operation cancelled: {info}"),
                    ts: time::OffsetDateTime::now_utc(),
                };
                ctx.chat_store.push(chat_item);
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
