use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use async_trait::async_trait;
use steer_grpc::client_api::ClientEvent;

pub struct QueueEventProcessor;

impl QueueEventProcessor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl EventProcessor for QueueEventProcessor {
    fn priority(&self) -> usize {
        15
    }

    fn can_handle(&self, event: &ClientEvent) -> bool {
        matches!(event, ClientEvent::QueueUpdated { .. })
    }

    async fn process(
        &mut self,
        event: ClientEvent,
        ctx: &mut ProcessingContext,
    ) -> ProcessingResult {
        match event {
            ClientEvent::QueueUpdated { head, count } => {
                *ctx.queued_head = head;
                *ctx.queued_count = count;
                ProcessingResult::Handled
            }
            _ => ProcessingResult::NotHandled,
        }
    }

    fn name(&self) -> &'static str {
        "QueueEventProcessor"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::events::processor::PendingToolApproval;
    use crate::tui::events::processor::ProcessingContext;
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
        in_flight_operations: std::collections::HashSet<steer_grpc::client_api::OpId>,
        queued_head: Option<QueuedWorkItem>,
        queued_count: usize,
        _workspace_root: tempfile::TempDir,
    }

    async fn create_test_context() -> TestContext {
        let chat_store = ChatStore::new();
        let chat_list_state = ChatListState::new();
        let tool_registry = ToolCallRegistry::new();
        let workspace_root = tempfile::TempDir::new().unwrap();
        let (client, _server_handle) = crate::tui::test_utils::local_client_and_server(
            None,
            Some(workspace_root.path().to_path_buf()),
        )
        .await;
        let is_processing = false;
        let progress_message = None;
        let spinner_state = 0;
        let current_tool_approval = None;
        let current_model = builtin::claude_sonnet_4_5();
        let current_agent_label = None;
        let messages_updated = false;
        let in_flight_operations = std::collections::HashSet::new();

        TestContext {
            chat_store,
            chat_list_state,
            tool_registry,
            client,
            input_panel_state: InputPanelState::new("test_session".to_string()),
            is_processing,
            progress_message,
            spinner_state,
            current_tool_approval,
            current_model,
            current_agent_label,
            messages_updated,
            in_flight_operations,
            queued_head: None,
            queued_count: 0,
            _workspace_root: workspace_root,
        }
    }

    #[tokio::test]
    async fn test_queue_updated_updates_state_and_clears_editing() {
        let mut processor = QueueEventProcessor::new();
        let mut ctx = create_test_context().await;

        let head = QueuedWorkItem {
            kind: QueuedWorkKind::UserMessage,
            content: "queued message".to_string(),
            model: None,
            queued_at: 123,
            op_id: OpId::new(),
            message_id: MessageId::from_string("msg_1"),
            attachment_count: 0,
        };

        let notification_manager =
            std::sync::Arc::new(crate::notifications::NotificationManager::new(
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
                ClientEvent::QueueUpdated {
                    head: Some(head.clone()),
                    count: 2,
                },
                &mut processing_ctx,
            )
            .await;

        assert!(matches!(result, ProcessingResult::Handled));
        assert_eq!(*processing_ctx.queued_count, 2);
        assert_eq!(processing_ctx.queued_head.as_ref(), Some(&head));

        let result = processor
            .process(
                ClientEvent::QueueUpdated {
                    head: None,
                    count: 0,
                },
                &mut processing_ctx,
            )
            .await;

        assert!(matches!(result, ProcessingResult::Handled));
        assert_eq!(*processing_ctx.queued_count, 0);
        assert!(processing_ctx.queued_head.is_none());
    }
}
