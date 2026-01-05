//! SystemEventProcessor - handles system and configuration events.
//!
//! Processes events related to model changes, command responses, and other
//! system-level state changes.

use crate::notifications::{NotificationConfig, NotificationSound, notify_with_sound};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::{ChatItemData, NoticeLevel, generate_row_id};
use async_trait::async_trait;
use steer_grpc::client_api::ClientEvent;

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
}

#[async_trait]
impl EventProcessor for SystemEventProcessor {
    fn priority(&self) -> usize {
        90 // Low priority - run after most other processors
    }

    fn can_handle(&self, event: &ClientEvent) -> bool {
        matches!(
            event,
            ClientEvent::ModelChanged { .. } | ClientEvent::Error { .. }
        )
    }

    async fn process(
        &mut self,
        event: ClientEvent,
        ctx: &mut ProcessingContext,
    ) -> ProcessingResult {
        match event {
            ClientEvent::ModelChanged { model } => {
                *ctx.current_model = model;
                ProcessingResult::Handled
            }
            ClientEvent::Error { message } => {
                let chat_item = crate::tui::model::ChatItem {
                    parent_chat_item_id: None,
                    data: ChatItemData::SystemNotice {
                        id: generate_row_id(),
                        level: NoticeLevel::Error,
                        text: message.clone(),
                        ts: time::OffsetDateTime::now_utc(),
                    },
                };
                ctx.chat_store.push(chat_item);
                *ctx.messages_updated = true;

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
