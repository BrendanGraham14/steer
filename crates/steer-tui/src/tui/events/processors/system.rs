//! SystemEventProcessor - handles system and configuration events.
//!
//! Processes events related to command responses, compaction, and other
//! system-level state changes.

use crate::notifications::{NotificationEvent, NotificationManager, NotificationManagerHandle};
use crate::tui::core_commands::{CommandResponse, CoreCommandType};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::{ChatItemData, NoticeLevel, generate_row_id};
use async_trait::async_trait;
use steer_grpc::client_api::{ClientEvent, CompactResult, CompactTrigger};

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
            ClientEvent::CompactResult { result, trigger } => {
                if matches!(result, CompactResult::Success(_)) {
                    ctx.llm_usage.clear();
                    *ctx.messages_updated = true;
                } else if matches!(trigger, CompactTrigger::Manual) {
                    // Manual non-success: show error/status via CoreCmdResponse
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
                }
                // Auto non-success: silent (no chat item)
                ProcessingResult::Handled
            }
            ClientEvent::ConversationCompacted { record } => {
                ctx.chat_store.mark_compaction_summary_with_head(
                    record.summary_message_id.to_string(),
                    Some(record.compacted_head_message_id.to_string()),
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::events::processor::{PendingToolApproval, ProcessingContext};
    use crate::tui::state::{ChatStore, LlmUsageState, ToolCallRegistry};
    use crate::tui::widgets::{ChatListState, input_panel::InputPanelState};
    use steer_grpc::AgentClient;
    use steer_grpc::client_api::{
        ContextWindowUsage, ModelId, OpId, Preferences, QueuedWorkItem, TokenUsage,
        UsageUpdateKind, builtin,
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
        llm_usage: LlmUsageState,
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
            is_processing: false,
            progress_message: None,
            spinner_state: 0,
            current_tool_approval: None,
            current_model: builtin::claude_sonnet_4_5(),
            current_agent_label: None,
            messages_updated: false,
            in_flight_operations: std::collections::HashSet::new(),
            queued_head: None,
            queued_count: 0,
            llm_usage: LlmUsageState::default(),
            _workspace_root: workspace_root,
        }
    }

    #[tokio::test]
    async fn compact_success_clears_ctx_usage() {
        let mut processor = SystemEventProcessor::new(std::sync::Arc::new(
            NotificationManager::new(&Preferences::default()),
        ));
        let mut ctx = create_test_context().await;

        let op_id = OpId::new();
        ctx.llm_usage.update(
            op_id,
            builtin::claude_sonnet_4_5(),
            TokenUsage::from_input_output(90, 30),
            Some(ContextWindowUsage {
                max_context_tokens: Some(128_000),
                remaining_tokens: Some(100_000),
                utilization_ratio: Some(0.21875),
                estimated: false,
            }),
            UsageUpdateKind::Final,
        );

        let notification_manager =
            std::sync::Arc::new(NotificationManager::new(&Preferences::default()));
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
            llm_usage: &mut ctx.llm_usage,
        };

        let result = processor
            .process(
                ClientEvent::CompactResult {
                    result: CompactResult::Success("summarized".to_string()),
                    trigger: CompactTrigger::Manual,
                },
                &mut processing_ctx,
            )
            .await;

        assert!(matches!(result, ProcessingResult::Handled));
        assert!(processing_ctx.llm_usage.latest().is_none());
    }

    #[tokio::test]
    async fn test_auto_compact_success_clears_usage_no_chat_item() {
        let mut processor = SystemEventProcessor::new(std::sync::Arc::new(
            NotificationManager::new(&Preferences::default()),
        ));
        let mut ctx = create_test_context().await;

        let notification_manager =
            std::sync::Arc::new(NotificationManager::new(&Preferences::default()));
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
            llm_usage: &mut ctx.llm_usage,
        };

        let result = processor
            .process(
                ClientEvent::CompactResult {
                    result: CompactResult::Success("summary".into()),
                    trigger: CompactTrigger::Auto,
                },
                &mut processing_ctx,
            )
            .await;

        assert!(matches!(result, ProcessingResult::Handled));
        assert!(*processing_ctx.messages_updated);
        // CompactResult no longer inserts a chat item; separator is handled
        // by ConversationCompacted marking the summary message.
        assert!(processing_ctx.chat_store.is_empty());
    }

    #[tokio::test]
    async fn test_auto_compact_cancelled_is_silent() {
        let mut processor = SystemEventProcessor::new(std::sync::Arc::new(
            NotificationManager::new(&Preferences::default()),
        ));
        let mut ctx = create_test_context().await;

        let notification_manager =
            std::sync::Arc::new(NotificationManager::new(&Preferences::default()));
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
            llm_usage: &mut ctx.llm_usage,
        };

        let result = processor
            .process(
                ClientEvent::CompactResult {
                    result: CompactResult::Cancelled,
                    trigger: CompactTrigger::Auto,
                },
                &mut processing_ctx,
            )
            .await;

        assert!(matches!(result, ProcessingResult::Handled));
        assert!(processing_ctx.chat_store.is_empty());
        assert!(!*processing_ctx.messages_updated);
    }

    #[tokio::test]
    async fn test_auto_compact_failed_is_silent() {
        let mut processor = SystemEventProcessor::new(std::sync::Arc::new(
            NotificationManager::new(&Preferences::default()),
        ));
        let mut ctx = create_test_context().await;

        let notification_manager =
            std::sync::Arc::new(NotificationManager::new(&Preferences::default()));
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
            llm_usage: &mut ctx.llm_usage,
        };

        let result = processor
            .process(
                ClientEvent::CompactResult {
                    result: CompactResult::Failed("error".into()),
                    trigger: CompactTrigger::Auto,
                },
                &mut processing_ctx,
            )
            .await;

        assert!(matches!(result, ProcessingResult::Handled));
        assert!(processing_ctx.chat_store.is_empty());
        assert!(!*processing_ctx.messages_updated);
    }
}
