use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use async_trait::async_trait;
use steer_grpc::client_api::ClientEvent;

pub struct UsageEventProcessor;

impl UsageEventProcessor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl EventProcessor for UsageEventProcessor {
    fn priority(&self) -> usize {
        20
    }

    fn can_handle(&self, event: &ClientEvent) -> bool {
        matches!(event, ClientEvent::LlmUsageUpdated { .. })
    }

    async fn process(
        &mut self,
        event: ClientEvent,
        ctx: &mut ProcessingContext,
    ) -> ProcessingResult {
        match event {
            ClientEvent::LlmUsageUpdated {
                op_id,
                model,
                usage,
                context_window,
                kind,
            } => {
                ctx.llm_usage
                    .update(op_id, model, usage, context_window, kind);
                ProcessingResult::Handled
            }
            _ => ProcessingResult::NotHandled,
        }
    }

    fn name(&self) -> &'static str {
        "UsageEventProcessor"
    }
}

impl Default for UsageEventProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::events::processor::PendingToolApproval;
    use crate::tui::events::processor::ProcessingContext;
    use crate::tui::state::{ChatStore, LlmUsageState, ToolCallRegistry};
    use crate::tui::widgets::{ChatListState, input_panel::InputPanelState};
    use std::collections::HashMap;
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
        notify_on_processing_complete: HashMap<OpId, bool>,
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
            notify_on_processing_complete: HashMap::new(),
            llm_usage: LlmUsageState::default(),
            _workspace_root: workspace_root,
        }
    }

    #[tokio::test]
    async fn llm_usage_updated_stores_latest_snapshot() {
        let mut processor = UsageEventProcessor::new();
        let mut ctx = create_test_context().await;

        let op_id = OpId::new();
        let model = builtin::claude_sonnet_4_5();
        let usage = TokenUsage::from_input_output(90, 30);

        let notification_manager = std::sync::Arc::new(
            crate::notifications::NotificationManager::new(&Preferences::default()),
        );

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
            notify_on_processing_complete: &mut ctx.notify_on_processing_complete,
            queued_head: &mut ctx.queued_head,
            queued_count: &mut ctx.queued_count,
            llm_usage: &mut ctx.llm_usage,
        };

        let result = processor
            .process(
                ClientEvent::LlmUsageUpdated {
                    op_id,
                    model: model.clone(),
                    usage,
                    context_window: Some(ContextWindowUsage {
                        max_context_tokens: Some(128_000),
                        remaining_tokens: Some(127_880),
                        utilization_ratio: Some(0.0009375),
                        estimated: false,
                    }),
                    kind: UsageUpdateKind::Final,
                },
                &mut processing_ctx,
            )
            .await;

        assert!(matches!(result, ProcessingResult::Handled));

        let latest = processing_ctx
            .llm_usage
            .latest()
            .expect("latest usage should be present");
        assert_eq!(latest.op_id, op_id);
        assert_eq!(latest.model, model);
        assert_eq!(latest.usage, usage);
        assert_eq!(latest.kind, UsageUpdateKind::Final);
        assert_eq!(latest.max_context_tokens, Some(128_000));
        assert_eq!(latest.remaining_tokens, Some(127_880));
        assert_eq!(latest.utilization_ratio, Some(0.0009375));
        assert!(!latest.context_estimated);
        assert!(!*processing_ctx.messages_updated);
    }

    #[tokio::test]
    async fn newer_usage_update_replaces_latest_snapshot_for_same_op() {
        let mut processor = UsageEventProcessor::new();
        let mut ctx = create_test_context().await;

        let op_id = OpId::new();
        let notification_manager = std::sync::Arc::new(
            crate::notifications::NotificationManager::new(&Preferences::default()),
        );

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
            notify_on_processing_complete: &mut ctx.notify_on_processing_complete,
            queued_head: &mut ctx.queued_head,
            queued_count: &mut ctx.queued_count,
            llm_usage: &mut ctx.llm_usage,
        };

        let _ = processor
            .process(
                ClientEvent::LlmUsageUpdated {
                    op_id,
                    model: builtin::claude_sonnet_4_5(),
                    usage: TokenUsage::from_input_output(80, 20),
                    context_window: Some(ContextWindowUsage {
                        max_context_tokens: Some(100_000),
                        remaining_tokens: Some(99_900),
                        utilization_ratio: Some(0.001),
                        estimated: true,
                    }),
                    kind: UsageUpdateKind::Partial,
                },
                &mut processing_ctx,
            )
            .await;

        let _ = processor
            .process(
                ClientEvent::LlmUsageUpdated {
                    op_id,
                    model: builtin::claude_sonnet_4_5(),
                    usage: TokenUsage::from_input_output(85, 25),
                    context_window: None,
                    kind: UsageUpdateKind::Final,
                },
                &mut processing_ctx,
            )
            .await;

        let latest = processing_ctx
            .llm_usage
            .latest()
            .expect("latest usage should be present");
        assert_eq!(latest.usage, TokenUsage::from_input_output(85, 25));
        assert_eq!(latest.kind, UsageUpdateKind::Final);
        assert_eq!(latest.max_context_tokens, None);
        assert_eq!(latest.remaining_tokens, None);
        assert_eq!(latest.utilization_ratio, None);
        assert!(!latest.context_estimated);

        let per_op = processing_ctx
            .llm_usage
            .for_op(&op_id)
            .expect("per-op usage should be present");
        assert_eq!(per_op.usage, TokenUsage::from_input_output(85, 25));
    }
}
