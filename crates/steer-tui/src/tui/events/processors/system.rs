//! SystemEventProcessor - handles system and configuration events.
//!
//! Processes events related to model changes, command responses, and other
//! system-level state changes.

use crate::notifications::{NotificationConfig, NotificationSound, notify_with_sound};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::{ChatItemData, NoticeLevel, generate_row_id};
use async_trait::async_trait;
use steer_core::app::AppEvent;

/// Processor for system-level events
pub struct SystemEventProcessor {
    notification_config: NotificationConfig,
}

impl SystemEventProcessor {
    pub fn new() -> Self {
        Self {
            notification_config: NotificationConfig::from_env(),
        }
    }

    /// Handle command response by adding it to the chat store
    fn handle_command_response(
        &self,
        command: steer_core::app::conversation::AppCommandType,
        response: steer_core::app::conversation::CommandResponse,
        ctx: &mut ProcessingContext,
    ) {
        let chat_item = crate::tui::model::ChatItem {
            parent_chat_item_id: None, // Will be set by push()
            data: ChatItemData::CoreCmdResponse {
                id: generate_row_id(),
                command,
                response,
                ts: time::OffsetDateTime::now_utc(),
            },
        };
        ctx.chat_store.push(chat_item);
    }
}

#[async_trait]
impl EventProcessor for SystemEventProcessor {
    fn priority(&self) -> usize {
        90 // Low priority - run after most other processors
    }

    fn can_handle(&self, event: &AppEvent) -> bool {
        matches!(
            event,
            AppEvent::ModelChanged { .. }
                | AppEvent::CommandResponse { .. }
                | AppEvent::Error { .. }
        )
    }

    async fn process(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> ProcessingResult {
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
            AppEvent::Error { message } => {
                // Add error as a system notice
                let chat_item = crate::tui::model::ChatItem {
                    parent_chat_item_id: None, // Will be set by push()
                    data: ChatItemData::SystemNotice {
                        id: generate_row_id(),
                        level: NoticeLevel::Error,
                        text: message.clone(),
                        ts: time::OffsetDateTime::now_utc(),
                    },
                };
                ctx.chat_store.push(chat_item);
                *ctx.messages_updated = true;

                // Trigger error notification with sound
                notify_with_sound(
                    &self.notification_config,
                    NotificationSound::Error,
                    &message,
                )
                .await;

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
