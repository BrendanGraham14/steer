//! ToolEventProcessor - handles tool call lifecycle events.
//!
//! Manages tool execution state, approval requests, completion, and failure events.

use crate::notifications::{NotificationConfig, NotificationSound, notify_with_sound};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::ChatItemData;
use async_trait::async_trait;
use steer_grpc::client_api::{
    ClientEvent, Message, MessageData, ToolCall, ToolCallId, ToolError, ToolResult,
};

/// Processor for tool-related events
pub struct ToolEventProcessor {
    notification_config: NotificationConfig,
}

impl ToolEventProcessor {
    pub fn new() -> Self {
        Self {
            notification_config: NotificationConfig::from_env(),
        }
    }
}

#[async_trait]
impl EventProcessor for ToolEventProcessor {
    fn priority(&self) -> usize {
        75 // After message events but before system events
    }

    fn can_handle(&self, event: &ClientEvent) -> bool {
        matches!(
            event,
            ClientEvent::ToolStarted { .. }
                | ClientEvent::ToolCompleted { .. }
                | ClientEvent::ToolFailed { .. }
                | ClientEvent::ApprovalRequested { .. }
        )
    }

    async fn process(
        &mut self,
        event: ClientEvent,
        ctx: &mut ProcessingContext,
    ) -> ProcessingResult {
        match event {
            ClientEvent::ToolStarted {
                name,
                id,
                parameters,
            } => {
                self.handle_tool_started(id, name, parameters, ctx);
                ProcessingResult::Handled
            }
            ClientEvent::ToolCompleted {
                name: _,
                result,
                id,
            } => {
                self.handle_tool_completed(id, result, ctx);
                ProcessingResult::Handled
            }
            ClientEvent::ToolFailed { name, error, id } => {
                self.handle_tool_failed(id, name, error, ctx);
                ProcessingResult::Handled
            }
            ClientEvent::ApprovalRequested {
                request_id,
                tool_call,
            } => {
                *ctx.current_tool_approval = Some((request_id, tool_call.clone()));

                notify_with_sound(
                    &self.notification_config,
                    NotificationSound::ToolApproval,
                    &format!("Tool approval needed: {}", tool_call.name),
                )
                .await;

                ProcessingResult::Handled
            }
            _ => ProcessingResult::NotHandled,
        }
    }

    fn name(&self) -> &'static str {
        "ToolEventProcessor"
    }
}

impl ToolEventProcessor {
    fn handle_tool_started(
        &self,
        id: ToolCallId,
        name: String,
        parameters: serde_json::Value,
        ctx: &mut ProcessingContext,
    ) {
        tracing::debug!(
            target: "tui.tool_event",
            "ToolStarted: id={}, name={}, parameters={:?}",
            id, name, parameters
        );

        ctx.tool_registry.debug_dump("At ToolStarted");

        *ctx.spinner_state = 0;
        *ctx.progress_message = Some(format!("Executing tool: {name}"));

        let tool_call = ToolCall {
            id: id.to_string(),
            name: name.clone(),
            parameters: parameters.clone(),
        };

        ctx.tool_registry.register_call(tool_call.clone());
        ctx.tool_registry.start_execution(id.as_str());

        let pending = crate::tui::model::ChatItem {
            parent_chat_item_id: None,
            data: ChatItemData::PendingToolCall {
                id: crate::tui::model::generate_row_id(),
                tool_call,
                ts: time::OffsetDateTime::now_utc(),
            },
        };

        ctx.chat_store.add_pending_tool(pending);
        *ctx.messages_updated = true;
    }

    fn handle_tool_completed(
        &self,
        id: ToolCallId, result: ToolResult, ctx: &mut ProcessingContext) {
        *ctx.progress_message = None;

        ctx.chat_store.remove_pending_tool(id.as_str());

        let tool_msg = Message {
            data: MessageData::Tool {
                tool_use_id: id.to_string(),
                result: result.clone(),
            },
            id: crate::tui::model::generate_row_id(),
            timestamp: chrono::Utc::now().timestamp() as u64,
            parent_message_id: None,
        };

        let _idx = ctx.chat_store.add_message(tool_msg);
        ctx.tool_registry.complete_execution(id.as_str(), result);
        *ctx.messages_updated = true;
    }

    fn handle_tool_failed(
        &self,
        id: ToolCallId,
        name: String,
        error: String,
        ctx: &mut ProcessingContext,
    ) {
        *ctx.progress_message = None;

        ctx.chat_store.remove_pending_tool(id.as_str());

        let tool_msg = Message {
            data: MessageData::Tool {
                tool_use_id: id.to_string(),
                result: ToolResult::Error(ToolError::Execution {
                    tool_name: name.clone(),
                    message: error.clone(),
                }),
            },
            id: crate::tui::model::generate_row_id(),
            timestamp: chrono::Utc::now().timestamp() as u64,
            parent_message_id: None,
        };

        let _idx = ctx.chat_store.add_message(tool_msg);
        ctx.tool_registry.fail_execution(id.as_str(), error);
        *ctx.messages_updated = true;
    }
}

impl Default for ToolEventProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::events::processor::ProcessingContext;
    use crate::tui::events::processors::message::MessageEventProcessor;
    use crate::tui::state::{ChatStore, ToolCallRegistry};
    use crate::tui::widgets::ChatListState;
    use steer_grpc::client_api::{AssistantContent, ModelId, OpId};

    use serde_json::json;
    use std::collections::HashSet;

    use crate::tui::events::processor::PendingToolApproval;
    use steer_grpc::AgentClient;

    struct TestContext {
        chat_store: ChatStore,
        chat_list_state: ChatListState,
        tool_registry: ToolCallRegistry,
        client: AgentClient,
        is_processing: bool,
        progress_message: Option<String>,
        spinner_state: usize,
        current_tool_approval: Option<PendingToolApproval>,
        current_model: ModelId,
        messages_updated: bool,
        in_flight_operations: HashSet<OpId>,
    }
    async fn create_test_context() -> TestContext {
        let chat_store = ChatStore::new();
        let chat_list_state = ChatListState::new();
        let tool_registry = ToolCallRegistry::new();
        let (client, _server_handle) = crate::tui::test_utils::local_client_and_server(None).await;
        let is_processing = false;
        let progress_message = None;
        let spinner_state = 0;
        let current_tool_approval = None;
        let current_model = steer_core::config::model::builtin::claude_3_5_sonnet_20241022();
        let messages_updated = false;
        let in_flight_operations = HashSet::new();
        TestContext {
            chat_store,
            chat_list_state,
            tool_registry,
            client,
            is_processing,
            progress_message,
            spinner_state,
            current_tool_approval,
            current_model,
            messages_updated,
            in_flight_operations,
        }
    }

    #[tokio::test]
    async fn test_toolcallstarted_after_assistant_keeps_params() {
        let mut tool_proc = ToolEventProcessor::new();
        let mut msg_proc = MessageEventProcessor::new();
        let mut ctx = create_test_context().await;

        let full_call = ToolCall {
            id: "id123".to_string(),
            name: "view".to_string(),
            parameters: json!({"file_path": "/tmp/x", "offset": 1}),
        };

        let assistant = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: full_call.clone(),
                }],
            },
            id: "a1".to_string(),
            timestamp: 0,
            parent_message_id: None,
        };

        let model = ctx.current_model.clone();

        {
            let mut ctx = ProcessingContext {
                chat_store: &mut ctx.chat_store,
                chat_list_state: &mut ctx.chat_list_state,
                tool_registry: &mut ctx.tool_registry,
                client: &ctx.client,
                is_processing: &mut ctx.is_processing,
                progress_message: &mut ctx.progress_message,
                spinner_state: &mut ctx.spinner_state,
                current_tool_approval: &mut ctx.current_tool_approval,
                current_model: &mut ctx.current_model,
                messages_updated: &mut ctx.messages_updated,
                in_flight_operations: &mut ctx.in_flight_operations,
            };
            let _ = msg_proc
                .process(
                    ClientEvent::AssistantMessageAdded {
                        message: assistant,
                        model: model.clone(),
                    },
                    &mut ctx,
                )
                .await;
        }

        {
            let mut ctx = ProcessingContext {
                chat_store: &mut ctx.chat_store,
                chat_list_state: &mut ctx.chat_list_state,
                tool_registry: &mut ctx.tool_registry,
                client: &ctx.client,
                is_processing: &mut ctx.is_processing,
                progress_message: &mut ctx.progress_message,
                spinner_state: &mut ctx.spinner_state,
                current_tool_approval: &mut ctx.current_tool_approval,
                current_model: &mut ctx.current_model,
                messages_updated: &mut ctx.messages_updated,
                in_flight_operations: &mut ctx.in_flight_operations,
            };
            let _ = tool_proc
                .process(
                    ClientEvent::ToolStarted {
                        name: "view".to_string(),
                        id: "id123".into(),
                        parameters: serde_json::Value::Null,
                    },
                    &mut ctx,
                )
                .await;
        }

        let stored_call = ctx
            .tool_registry
            .get_tool_call("id123")
            .expect("tool call should exist");
        assert_eq!(stored_call.parameters, full_call.parameters);
        assert_eq!(stored_call.name, "view");
        assert_eq!(stored_call.id, "id123");
    }
}
