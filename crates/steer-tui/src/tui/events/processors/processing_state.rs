//! ProcessingStateProcessor - handles processing state events.
//!
//! Manages the overall processing state of the TUI, including thinking/processing
//! indicators, spinner state, progress messages, and completion notifications.

use crate::notifications::{NotificationConfig, NotificationSound, notify_with_sound};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use async_trait::async_trait;
use steer_grpc::client_api::ClientEvent;

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

#[async_trait]
impl EventProcessor for ProcessingStateProcessor {
    fn priority(&self) -> usize {
        10 // High priority - state changes should happen early
    }

    fn can_handle(&self, event: &ClientEvent) -> bool {
        matches!(
            event,
            ClientEvent::ProcessingStarted { .. }
                | ClientEvent::ProcessingCompleted { .. }
                | ClientEvent::Error { .. }
                | ClientEvent::OperationCancelled { .. }
        )
    }

    async fn process(
        &mut self,
        event: ClientEvent,
        ctx: &mut ProcessingContext,
    ) -> ProcessingResult {
        match event {
            ClientEvent::ProcessingStarted { op_id } => {
                *ctx.is_processing = true;
                *ctx.spinner_state = 0;
                ctx.in_flight_operations.insert(op_id);
                ProcessingResult::Handled
            }
            ClientEvent::ProcessingCompleted { op_id } => {
                let was_processing = *ctx.is_processing;
                *ctx.is_processing = false;
                *ctx.progress_message = None;
                ctx.in_flight_operations.remove(&op_id);

                if was_processing {
                    notify_with_sound(
                        &self.notification_config,
                        NotificationSound::ProcessingComplete,
                        "Processing complete - waiting for input",
                    )
                    .await;
                }

                ProcessingResult::Handled
            }
            ClientEvent::Error { message } => {
                let was_processing = *ctx.is_processing;
                *ctx.is_processing = false;
                *ctx.progress_message = None;

                if was_processing {
                    notify_with_sound(
                        &self.notification_config,
                        NotificationSound::Error,
                        &message,
                    )
                    .await;
                }

                ProcessingResult::Handled
            }
            ClientEvent::OperationCancelled {
                op_id,
                pending_tool_calls: _,
            } => {
                *ctx.is_processing = false;
                *ctx.progress_message = None;
                *ctx.current_tool_approval = None;
                ctx.in_flight_operations.remove(&op_id);

                let chat_item = crate::tui::model::ChatItem {
                    parent_chat_item_id: None,
                    data: crate::tui::model::ChatItemData::SystemNotice {
                        id: crate::tui::model::generate_row_id(),
                        level: crate::tui::model::NoticeLevel::Info,
                        text: "Operation cancelled".to_string(),
                        ts: time::OffsetDateTime::now_utc(),
                    },
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
