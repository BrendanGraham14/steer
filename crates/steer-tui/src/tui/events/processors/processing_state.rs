//! ProcessingStateProcessor - handles processing state events.
//!
//! Manages the overall processing state of the TUI, including thinking/processing
//! indicators, spinner state, progress messages, and completion notifications.

use crate::notifications::{NotificationConfig, NotificationSound, notify_with_sound};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use async_trait::async_trait;
use steer_core::app::AppEvent;

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

    fn can_handle(&self, event: &AppEvent) -> bool {
        matches!(
            event,
            AppEvent::ProcessingStarted
                | AppEvent::ProcessingCompleted
                | AppEvent::Error { .. }
                | AppEvent::OperationCancelled { .. }
                | AppEvent::Started {
                    op: steer_core::app::Operation::Bash { .. },
                    ..
                }
                | AppEvent::Started {
                    op: steer_core::app::Operation::Compact,
                    ..
                }
                | AppEvent::Finished {
                    outcome: steer_core::app::OperationOutcome::Bash { .. },
                    ..
                }
                | AppEvent::Finished {
                    outcome: steer_core::app::OperationOutcome::Compact { .. },
                    ..
                }
        )
    }

    async fn process(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> ProcessingResult {
        match event {
            AppEvent::ProcessingStarted => {
                *ctx.is_processing = true;
                *ctx.spinner_state = 0;
                ProcessingResult::Handled
            }
            AppEvent::ProcessingCompleted => {
                let was_processing = *ctx.is_processing;
                *ctx.is_processing = false;
                *ctx.progress_message = None;

                // Trigger notification if we were processing
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
            AppEvent::Error { .. } => {
                let was_processing = *ctx.is_processing;
                *ctx.is_processing = false;
                *ctx.progress_message = None;

                // Trigger error notification if we were processing
                if was_processing {
                    notify_with_sound(
                        &self.notification_config,
                        NotificationSound::Error,
                        "An error occurred",
                    )
                    .await;
                }

                ProcessingResult::Handled
            }
            AppEvent::OperationCancelled { op_id: _, info: _ } => {
                *ctx.is_processing = false;
                *ctx.progress_message = None;
                *ctx.current_tool_approval = None;

                // Add cancellation message to the UI
                let chat_item = crate::tui::model::ChatItem {
                    parent_chat_item_id: None, // Will be set by push()
                    data: crate::tui::model::ChatItemData::SystemNotice {
                        id: crate::tui::model::generate_row_id(),
                        level: crate::tui::model::NoticeLevel::Info,
                        text: "Operation cancelled".to_string(),
                        ts: time::OffsetDateTime::now_utc(),
                    },
                };
                ctx.chat_store.push(chat_item);
                *ctx.messages_updated = true;

                // Remove any in-flight operation rows (stops spinners)
                let operations_to_remove: Vec<uuid::Uuid> =
                    ctx.in_flight_operations.iter().cloned().collect();
                ctx.in_flight_operations.clear();

                for operation_id in operations_to_remove {
                    ctx.chat_store.remove_in_flight_op(&operation_id);
                    *ctx.messages_updated = true;
                }

                ProcessingResult::Handled
            }
            AppEvent::Started { id, op } => {
                // Only handle non-tool operations
                let label = match op {
                    steer_core::app::Operation::Bash { cmd } => {
                        format!("Running command: {cmd}")
                    }
                    steer_core::app::Operation::Compact => {
                        "Compressing conversation...".to_string()
                    }
                };

                // Add in-flight operation row
                let row_id = crate::tui::model::generate_row_id();
                let chat_item = crate::tui::model::ChatItem {
                    parent_chat_item_id: None, // Will be set by push()
                    data: crate::tui::model::ChatItemData::InFlightOperation {
                        id: row_id.clone(),
                        operation_id: id,
                        label,
                        ts: time::OffsetDateTime::now_utc(),
                    },
                };
                ctx.chat_store.push(chat_item);
                *ctx.messages_updated = true;
                ctx.in_flight_operations.insert(id);

                ProcessingResult::Handled
            }
            AppEvent::Finished { id, outcome: _ } => {
                // Remove the in-flight operation if it exists
                if ctx.in_flight_operations.remove(&id) {
                    ctx.chat_store.remove_in_flight_op(&id);
                    *ctx.messages_updated = true;
                }

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
