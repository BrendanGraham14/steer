//! ProcessingStateProcessor - handles processing state events.
//!
//! Manages the overall processing state of the TUI, including thinking/processing
//! indicators, spinner state, progress messages, and completion notifications.

use crate::notifications::{NotificationEvent, NotificationManager, NotificationManagerHandle};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use async_trait::async_trait;
use steer_grpc::client_api::ClientEvent;

/// Processor for events that affect the overall processing state
pub struct ProcessingStateProcessor {
    notification_manager: NotificationManagerHandle,
}

impl ProcessingStateProcessor {
    pub fn new(notification_manager: NotificationManagerHandle) -> Self {
        Self {
            notification_manager,
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
                    self.notification_manager
                        .emit(NotificationEvent::ProcessingComplete);
                }

                ProcessingResult::Handled
            }
            ClientEvent::Error { message } => {
                let was_processing = *ctx.is_processing;
                *ctx.is_processing = false;
                *ctx.progress_message = None;

                if was_processing {
                    self.notification_manager.emit(NotificationEvent::Error {
                        message: message.clone(),
                    });
                }

                ProcessingResult::Handled
            }
            ClientEvent::OperationCancelled {
                op_id,
                pending_tool_calls: _,
                popped_queued_item,
            } => {
                *ctx.is_processing = false;
                *ctx.progress_message = None;
                *ctx.current_tool_approval = None;
                ctx.in_flight_operations.remove(&op_id);

                if let Some(item) = popped_queued_item {
                    let content = match item.kind {
                        steer_grpc::client_api::QueuedWorkKind::DirectBash => {
                            format!("!{}", item.content)
                        }
                        _ => item.content,
                    };
                    ctx.input_panel_state.replace_content(&content, None);
                }

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
        Self::new(std::sync::Arc::new(NotificationManager::new(
            &steer_grpc::client_api::Preferences::default(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::events::processor::{PendingToolApproval, ProcessingContext};
    use crate::tui::state::{ChatStore, ToolCallRegistry};
    use crate::tui::widgets::{ChatListState, input_panel::InputPanelState};
    use steer_grpc::AgentClient;
    use steer_grpc::client_api::{
        MessageId, ModelId, OpId, QueuedWorkItem, QueuedWorkKind, builtin,
    };

    struct TestContext {
        chat_store: ChatStore,
        chat_list_state: ChatListState,
        tool_registry: ToolCallRegistry,
        client: AgentClient,
        input_panel_state: InputPanelState,
        is_processing: bool,
        progress_message: Option<String>,
        spinner_state: usize,
        current_tool_approval: Option<PendingToolApproval>,
        current_model: ModelId,
        current_agent_label: Option<String>,
        messages_updated: bool,
        in_flight_operations: std::collections::HashSet<OpId>,
        queued_head: Option<QueuedWorkItem>,
        queued_count: usize,
        _workspace_root: tempfile::TempDir,
    }

    async fn create_test_context() -> TestContext {
        let chat_store = ChatStore::new();
        let chat_list_state = ChatListState::new();
        let tool_registry = ToolCallRegistry::new();
        let workspace_root = tempfile::TempDir::new().expect("tempdir");
        let (client, _server_handle) = crate::tui::test_utils::local_client_and_server(
            None,
            Some(workspace_root.path().to_path_buf()),
        )
        .await;

        TestContext {
            chat_store,
            chat_list_state,
            tool_registry,
            client,
            input_panel_state: InputPanelState::new("test_session".to_string()),
            is_processing: true,
            progress_message: Some("working".to_string()),
            spinner_state: 1,
            current_tool_approval: None,
            current_model: builtin::claude_sonnet_4_5(),
            current_agent_label: None,
            messages_updated: false,
            in_flight_operations: std::collections::HashSet::new(),
            queued_head: None,
            queued_count: 0,
            _workspace_root: workspace_root,
        }
    }

    #[tokio::test]
    async fn operation_cancelled_restores_popped_queue_item_to_input() {
        let mut processor = ProcessingStateProcessor::default();
        let mut ctx = create_test_context().await;

        let op_id = OpId::new();
        ctx.in_flight_operations.insert(op_id);

        let popped = QueuedWorkItem {
            kind: QueuedWorkKind::UserMessage,
            content: "queued draft".to_string(),
            model: None,
            queued_at: 123,
            op_id: OpId::new(),
            message_id: MessageId::from_string("msg_queued"),
        };

        let notification_manager = std::sync::Arc::new(NotificationManager::new(
            &steer_grpc::client_api::Preferences::default(),
        ));

        let mut processing_ctx = ProcessingContext {
            chat_store: &mut ctx.chat_store,
            chat_list_state: &mut ctx.chat_list_state,
            tool_registry: &mut ctx.tool_registry,
            client: &ctx.client,
            notification_manager: &notification_manager,
            input_panel_state: &mut ctx.input_panel_state,
            is_processing: &mut ctx.is_processing,
            progress_message: &mut ctx.progress_message,
            spinner_state: &mut ctx.spinner_state,
            current_tool_approval: &mut ctx.current_tool_approval,
            current_model: &mut ctx.current_model,
            current_agent_label: &mut ctx.current_agent_label,
            messages_updated: &mut ctx.messages_updated,
            in_flight_operations: &mut ctx.in_flight_operations,
            queued_head: &mut ctx.queued_head,
            queued_count: &mut ctx.queued_count,
        };

        let result = processor
            .process(
                ClientEvent::OperationCancelled {
                    op_id,
                    pending_tool_calls: 0,
                    popped_queued_item: Some(popped),
                },
                &mut processing_ctx,
            )
            .await;

        assert!(matches!(result, ProcessingResult::Handled));
        assert_eq!(processing_ctx.input_panel_state.content(), "queued draft");
    }

    #[tokio::test]
    async fn operation_cancelled_restores_bash_with_prefix() {
        let mut processor = ProcessingStateProcessor::default();
        let mut ctx = create_test_context().await;

        let op_id = OpId::new();
        ctx.in_flight_operations.insert(op_id);

        let popped = QueuedWorkItem {
            kind: QueuedWorkKind::DirectBash,
            content: "ls -la".to_string(),
            model: None,
            queued_at: 123,
            op_id: OpId::new(),
            message_id: MessageId::from_string("msg_bash"),
        };

        let notification_manager = std::sync::Arc::new(NotificationManager::new(
            &steer_grpc::client_api::Preferences::default(),
        ));

        let mut processing_ctx = ProcessingContext {
            chat_store: &mut ctx.chat_store,
            chat_list_state: &mut ctx.chat_list_state,
            tool_registry: &mut ctx.tool_registry,
            client: &ctx.client,
            notification_manager: &notification_manager,
            input_panel_state: &mut ctx.input_panel_state,
            is_processing: &mut ctx.is_processing,
            progress_message: &mut ctx.progress_message,
            spinner_state: &mut ctx.spinner_state,
            current_tool_approval: &mut ctx.current_tool_approval,
            current_model: &mut ctx.current_model,
            current_agent_label: &mut ctx.current_agent_label,
            messages_updated: &mut ctx.messages_updated,
            in_flight_operations: &mut ctx.in_flight_operations,
            queued_head: &mut ctx.queued_head,
            queued_count: &mut ctx.queued_count,
        };

        let _ = processor
            .process(
                ClientEvent::OperationCancelled {
                    op_id,
                    pending_tool_calls: 0,
                    popped_queued_item: Some(popped),
                },
                &mut processing_ctx,
            )
            .await;

        assert_eq!(processing_ctx.input_panel_state.content(), "!ls -la");
    }
}
