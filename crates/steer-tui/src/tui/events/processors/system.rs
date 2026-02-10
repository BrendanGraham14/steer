//! SystemEventProcessor - handles system and configuration events.
//!
//! Processes events related to command responses, compaction, and other
//! system-level state changes.

use crate::notifications::{NotificationEvent, NotificationManager, NotificationManagerHandle};
use crate::tui::core_commands::{CommandResponse, CoreCommandType};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::{ChatItemData, NoticeLevel, generate_row_id};
use async_trait::async_trait;
use steer_grpc::client_api::ClientEvent;

/// Processor for system-level events
pub struct SystemEventProcessor {
    notification_manager: NotificationManagerHandle,
}

impl SystemEventProcessor {
    pub fn new(notification_manager: NotificationManagerHandle) -> Self {
        Self {
            notification_manager,
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
            ClientEvent::Error { .. }
                | ClientEvent::CompactResult { .. }
                | ClientEvent::ConversationCompacted { .. }
                | ClientEvent::SessionConfigUpdated { .. }
        )
    }

    async fn process(
        &mut self,
        event: ClientEvent,
        ctx: &mut ProcessingContext,
    ) -> ProcessingResult {
        match event {
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

                self.notification_manager.emit(NotificationEvent::Error {
                    message: message.clone(),
                });

                ProcessingResult::Handled
            }
            ClientEvent::CompactResult { result } => {
                let chat_item = crate::tui::model::ChatItem {
                    parent_chat_item_id: None,
                    data: ChatItemData::CoreCmdResponse {
                        id: generate_row_id(),
                        command: CoreCommandType::Compact,
                        response: CommandResponse::Compact(result),
                        ts: time::OffsetDateTime::now_utc(),
                    },
                };
                ctx.chat_store.push(chat_item);
                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            ClientEvent::ConversationCompacted { record } => {
                ctx.chat_store
                    .set_compaction_head(Some(record.compacted_head_message_id.to_string()));
                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            ClientEvent::SessionConfigUpdated {
                primary_agent_id,
                config: _,
            } => {
                let label = crate::tui::format_agent_label(&primary_agent_id);
                *ctx.current_agent_label = Some(label);
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
        Self::new(std::sync::Arc::new(NotificationManager::new(
            &steer_grpc::client_api::Preferences::default(),
        )))
    }
}
