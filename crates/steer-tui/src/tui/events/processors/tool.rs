//! ToolEventProcessor - handles tool call lifecycle events.
//!
//! Manages tool execution state, approval requests, completion, and failure events.

use crate::notifications::{NotificationConfig, NotificationSound, notify_with_sound};
use crate::tui::events::processor::{EventProcessor, ProcessingContext, ProcessingResult};
use crate::tui::model::ChatItemData;
use async_trait::async_trait;
use steer_core::app::AppEvent;
use steer_core::app::conversation::ToolResult;
use steer_tools::error::ToolError;

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

    fn can_handle(&self, event: &AppEvent) -> bool {
        matches!(
            event,
            AppEvent::ToolCallStarted { .. }
                | AppEvent::ToolCallCompleted { .. }
                | AppEvent::ToolCallFailed { .. }
                | AppEvent::RequestToolApproval { .. }
        )
    }

    async fn process(&mut self, event: AppEvent, ctx: &mut ProcessingContext) -> ProcessingResult {
        match event {
            AppEvent::ToolCallStarted {
                name,
                id,
                parameters,
                ..
            } => {
                tracing::debug!(
                    target: "tui.tool_event",
                    "ToolCallStarted: id={}, name={}, parameters={:?}",
                    id, name, parameters
                );

                // Debug: dump registry state
                ctx.tool_registry.debug_dump("At ToolCallStarted");

                *ctx.spinner_state = 0;
                *ctx.progress_message = Some(format!("Executing tool: {name}"));

                // Create a ToolCall struct with the parameters
                let tool_call = steer_tools::schema::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    parameters: parameters.clone(),
                };

                // Store the tool call in the registry with full parameters
                ctx.tool_registry.register_call(tool_call.clone());

                // Add a pending tool call item
                let pending = crate::tui::model::ChatItem {
                    parent_chat_item_id: None, // Will be set by push()
                    data: ChatItemData::PendingToolCall {
                        id: crate::tui::model::generate_row_id(),
                        tool_call,
                        ts: time::OffsetDateTime::now_utc(),
                    },
                };

                ctx.chat_store.add_pending_tool(pending);

                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            AppEvent::ToolCallCompleted {
                name: _,
                result,
                id,
                ..
            } => {
                *ctx.progress_message = None;

                ctx.chat_store.remove_pending_tool(&id);

                // Create a complete tool message
                let tool_msg = steer_core::app::conversation::Message {
                    data: steer_core::app::conversation::MessageData::Tool {
                        tool_use_id: id.clone(),
                        result: result.clone(),
                    },
                    id: crate::tui::model::generate_row_id(),
                    timestamp: chrono::Utc::now().timestamp() as u64,
                    parent_message_id: None,
                };

                let _idx = ctx.chat_store.add_message(tool_msg);

                // Complete the tool execution in registry
                ctx.tool_registry.complete_execution(&id, result);

                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            AppEvent::ToolCallFailed {
                name, error, id, ..
            } => {
                *ctx.progress_message = None;

                ctx.chat_store.remove_pending_tool(&id);

                // Create a complete tool message with error
                let tool_msg = steer_core::app::conversation::Message {
                    data: steer_core::app::conversation::MessageData::Tool {
                        tool_use_id: id.clone(),
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

                // Complete the tool execution in registry with error
                ctx.tool_registry.fail_execution(&id, error);

                *ctx.messages_updated = true;
                ProcessingResult::Handled
            }
            AppEvent::RequestToolApproval {
                name,
                parameters,
                id,
                ..
            } => {
                let approval_info = steer_tools::schema::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    parameters: parameters.clone(),
                };

                *ctx.current_tool_approval = Some(approval_info);

                // Notify user about tool approval request
                notify_with_sound(
                    &self.notification_config,
                    NotificationSound::ToolApproval,
                    &format!("Tool approval needed: {name}"),
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

    use serde_json::json;
    use std::collections::HashSet;

    use steer_core::app::conversation::{AssistantContent, Message, MessageData};

    use steer_grpc::AgentClient;
    use steer_tools::schema::ToolCall;

    struct TestContext {
        chat_store: ChatStore,
        chat_list_state: ChatListState,
        tool_registry: ToolCallRegistry,
        client: AgentClient,
        is_processing: bool,
        progress_message: Option<String>,
        spinner_state: usize,
        current_tool_approval: Option<ToolCall>,
        current_model: steer_core::api::Model,
        messages_updated: bool,
        in_flight_operations: HashSet<uuid::Uuid>,
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
        let current_model = steer_core::api::Model::Claude3_5Sonnet20241022;
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

        // Full tool call we expect to keep
        let full_call = ToolCall {
            id: "id123".to_string(),
            name: "view".to_string(),
            parameters: json!({"file_path": "/tmp/x", "offset": 1}),
        };

        // 1. Assistant message first - populates registry with full params
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

        let model = ctx.current_model;

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
                    steer_core::app::AppEvent::MessageAdded {
                        message: assistant,
                        model,
                    },
                    &mut ctx,
                )
                .await;
        }

        // 2. ToolCallStarted arrives afterwards
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
                    steer_core::app::AppEvent::ToolCallStarted {
                        name: "view".to_string(),
                        id: "id123".to_string(),
                        parameters: serde_json::Value::Null,
                        model,
                    },
                    &mut ctx,
                )
                .await;
        }

        // Assert the tool was registered and stored properly
        let stored_call = ctx
            .tool_registry
            .get_tool_call("id123")
            .expect("tool call should exist");
        assert_eq!(stored_call.parameters, full_call.parameters);
        assert_eq!(stored_call.name, "view");
        assert_eq!(stored_call.id, "id123");
    }
}
