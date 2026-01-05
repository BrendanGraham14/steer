use crate::app::conversation::{AssistantContent, Message, MessageData, UserContent};
use crate::app::domain::action::{Action, ApprovalDecision, ApprovalMemory};
use crate::app::domain::effect::Effect;
use crate::app::domain::event::{CancellationInfo, SessionEvent};
use crate::app::domain::state::{AppState, OperationKind, PendingApproval, QueuedApproval};
use steer_tools::ToolError;
use steer_tools::result::ToolResult;
use steer_tools::tools::BASH_TOOL_NAME;

pub fn reduce(state: &mut AppState, action: Action) -> Vec<Effect> {
    match action {
        Action::UserInput {
            session_id,
            text,
            op_id,
            message_id,
            timestamp,
        } => handle_user_input(state, session_id, text, op_id, message_id, timestamp),

        Action::UserEditedMessage {
            session_id,
            message_id,
            new_content,
            op_id,
            new_message_id,
            timestamp,
        } => handle_user_edited_message(
            state,
            session_id,
            message_id,
            new_content,
            op_id,
            new_message_id,
            timestamp,
        ),

        Action::SlashCommand {
            session_id,
            command,
            timestamp,
        } => handle_slash_command(state, session_id, command, timestamp),

        Action::ToolApprovalRequested {
            session_id,
            request_id,
            tool_call,
        } => handle_tool_approval_requested(state, session_id, request_id, tool_call),

        Action::ToolApprovalDecided {
            session_id,
            request_id,
            decision,
            remember,
        } => handle_tool_approval_decided(state, session_id, request_id, decision, remember),

        Action::ToolExecutionStarted {
            session_id,
            tool_call_id,
            tool_name,
            tool_parameters,
        } => handle_tool_execution_started(
            state,
            session_id,
            tool_call_id,
            tool_name,
            tool_parameters,
        ),

        Action::ToolResult {
            session_id,
            tool_call_id,
            tool_name,
            result,
        } => handle_tool_result(state, session_id, tool_call_id, tool_name, result),

        Action::ModelResponseComplete {
            session_id,
            op_id,
            message_id,
            content,
            timestamp,
        } => {
            handle_model_response_complete(state, session_id, op_id, message_id, content, timestamp)
        }

        Action::ModelResponseError {
            session_id,
            op_id,
            error,
        } => handle_model_response_error(state, session_id, op_id, &error),

        Action::Cancel { session_id, op_id } => handle_cancel(state, session_id, op_id),

        Action::Hydrate {
            session_id,
            events,
            starting_sequence,
        } => handle_hydrate(state, session_id, events, starting_sequence),

        Action::WorkspaceFilesListed { files, .. } => {
            state.workspace_files = files;
            vec![]
        }

        Action::ToolSchemasAvailable { tools, .. } => {
            state.tools = tools;
            vec![]
        }

        Action::ToolSchemasUpdated { schemas, .. } => {
            state.tools = schemas;
            vec![]
        }

        Action::McpServerStateChanged {
            server_name,
            state: new_state,
            ..
        } => {
            state.mcp_servers.insert(server_name, new_state);
            vec![]
        }

        Action::Shutdown => vec![],
    }
}

fn handle_user_input(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    text: crate::app::domain::types::NonEmptyString,
    op_id: crate::app::domain::types::OpId,
    message_id: crate::app::domain::types::MessageId,
    timestamp: u64,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    let parent_id = state.conversation.active_message_id.clone();

    let message = Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: text.as_str().to_string(),
            }],
        },
        timestamp,
        id: message_id.0.clone(),
        parent_message_id: parent_id,
    };

    state.conversation.add_message(message.clone());
    state.conversation.active_message_id = Some(message_id.0.clone());

    state.start_operation(op_id, OperationKind::AgentLoop);

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::MessageAdded {
            message: message.clone(),
            model: state.current_model.clone(),
        },
    });

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::OperationStarted {
            op_id,
            kind: OperationKind::AgentLoop,
        },
    });

    effects.push(Effect::CallModel {
        session_id,
        op_id,
        model: state.current_model.clone(),
        messages: state
            .conversation
            .get_thread_messages()
            .into_iter()
            .cloned()
            .collect(),
        system_prompt: state.cached_system_prompt.clone(),
        tools: state.tools.clone(),
    });

    effects
}

fn handle_user_edited_message(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    original_message_id: crate::app::domain::types::MessageId,
    new_content: String,
    op_id: crate::app::domain::types::OpId,
    new_message_id: crate::app::domain::types::MessageId,
    timestamp: u64,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    let parent_id = state
        .conversation
        .messages
        .iter()
        .find(|m| m.id() == original_message_id.0)
        .and_then(|m| m.parent_message_id().map(|s| s.to_string()));

    let message = Message {
        data: MessageData::User {
            content: vec![UserContent::Text { text: new_content }],
        },
        timestamp,
        id: new_message_id.0.clone(),
        parent_message_id: parent_id,
    };

    state.conversation.add_message(message.clone());
    state.conversation.active_message_id = Some(new_message_id.0.clone());

    state.start_operation(op_id, OperationKind::AgentLoop);

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::MessageAdded {
            message: message.clone(),
            model: state.current_model.clone(),
        },
    });

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::OperationStarted {
            op_id,
            kind: OperationKind::AgentLoop,
        },
    });

    effects.push(Effect::CallModel {
        session_id,
        op_id,
        model: state.current_model.clone(),
        messages: state
            .conversation
            .get_thread_messages()
            .into_iter()
            .cloned()
            .collect(),
        system_prompt: state.cached_system_prompt.clone(),
        tools: state.tools.clone(),
    });

    effects
}

fn handle_slash_command(
    _state: &mut AppState,
    _session_id: crate::app::domain::types::SessionId,
    _command: crate::app::conversation::AppCommandType,
    _timestamp: u64,
) -> Vec<Effect> {
    vec![]
}

fn handle_tool_approval_requested(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    request_id: crate::app::domain::types::RequestId,
    tool_call: steer_tools::ToolCall,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    if is_pre_approved(state, &tool_call) {
        let op_id = state
            .current_operation
            .as_ref()
            .map(|o| o.op_id)
            .expect("Operation should exist");

        effects.push(Effect::ExecuteTool {
            session_id,
            op_id,
            tool_call,
        });
        return effects;
    }

    if state.pending_approval.is_some() {
        state.approval_queue.push_back(QueuedApproval { tool_call });
        return effects;
    }

    state.pending_approval = Some(PendingApproval {
        request_id,
        tool_call: tool_call.clone(),
    });

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::ApprovalRequested {
            request_id,
            tool_call: tool_call.clone(),
        },
    });

    effects.push(Effect::RequestUserApproval {
        session_id,
        request_id,
        tool_call,
    });

    effects
}

fn handle_tool_approval_decided(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    request_id: crate::app::domain::types::RequestId,
    decision: ApprovalDecision,
    remember: Option<ApprovalMemory>,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    let pending = match state.pending_approval.take() {
        Some(p) if p.request_id == request_id => p,
        other => {
            state.pending_approval = other;
            return effects;
        }
    };

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::ApprovalDecided {
            request_id,
            decision,
            remember: remember.clone(),
        },
    });

    if decision == ApprovalDecision::Approved {
        if let Some(ref memory) = remember {
            match memory {
                ApprovalMemory::Tool(name) => {
                    state.approved_tools.insert(name.clone());
                }
                ApprovalMemory::BashPattern(pattern) => {
                    state.approved_bash_patterns.insert(pattern.clone());
                }
            }
        }

        let op_id = state
            .current_operation
            .as_ref()
            .map(|o| o.op_id)
            .expect("Operation should exist");

        effects.push(Effect::ExecuteTool {
            session_id,
            op_id,
            tool_call: pending.tool_call,
        });
    }

    effects.extend(process_next_queued_approval(state, session_id));

    effects
}

fn process_next_queued_approval(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    while let Some(queued) = state.approval_queue.pop_front() {
        if is_pre_approved(state, &queued.tool_call) {
            let op_id = state
                .current_operation
                .as_ref()
                .map(|o| o.op_id)
                .expect("Operation should exist");

            effects.push(Effect::ExecuteTool {
                session_id,
                op_id,
                tool_call: queued.tool_call,
            });
            continue;
        }

        let request_id = crate::app::domain::types::RequestId::new();
        state.pending_approval = Some(PendingApproval {
            request_id,
            tool_call: queued.tool_call.clone(),
        });

        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::ApprovalRequested {
                request_id,
                tool_call: queued.tool_call.clone(),
            },
        });

        effects.push(Effect::RequestUserApproval {
            session_id,
            request_id,
            tool_call: queued.tool_call,
        });

        break;
    }

    effects
}

fn is_pre_approved(state: &AppState, tool_call: &steer_tools::ToolCall) -> bool {
    if state.approved_tools.contains(&tool_call.name) {
        return true;
    }

    if tool_call.name == BASH_TOOL_NAME
        && let Ok(params) = serde_json::from_value::<steer_tools::tools::bash::BashParams>(
            tool_call.parameters.clone(),
        ) {
            return state.is_bash_pattern_approved(&params.command);
        }
    }

    false
}

fn handle_tool_execution_started(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    tool_call_id: crate::app::domain::types::ToolCallId,
    tool_name: String,
    tool_parameters: serde_json::Value,
) -> Vec<Effect> {
    state.add_pending_tool_call(tool_call_id.clone());

    vec![Effect::EmitEvent {
        session_id,
        event: SessionEvent::ToolCallStarted {
            id: tool_call_id,
            name: tool_name,
            parameters: tool_parameters,
        },
    }]
}

fn handle_tool_result(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    tool_call_id: crate::app::domain::types::ToolCallId,
    tool_name: String,
    result: Result<ToolResult, ToolError>,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    if let Some(op) = &state.current_operation {
        if state.cancelled_ops.contains(&op.op_id) {
            tracing::debug!("Ignoring late tool result for cancelled op {:?}", op.op_id);
            return effects;
        }
    }

    state.remove_pending_tool_call(&tool_call_id);

    let event = match &result {
        Ok(tool_result) => SessionEvent::ToolCallCompleted {
            id: tool_call_id.clone(),
            name: String::new(),
            result: tool_result.clone(),
        },
        Err(e) => SessionEvent::ToolCallFailed {
            id: tool_call_id.clone(),
            name: tool_name.clone(),
            error: e.to_string(),
        },
    };

    effects.push(Effect::EmitEvent { session_id, event });

    effects
}

fn handle_model_response_complete(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    op_id: crate::app::domain::types::OpId,
    message_id: crate::app::domain::types::MessageId,
    content: Vec<AssistantContent>,
    timestamp: u64,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    if state.cancelled_ops.contains(&op_id) {
        tracing::debug!("Ignoring model response for cancelled op {:?}", op_id);
        return effects;
    }

    let parent_id = state.conversation.active_message_id.clone();

    let message = Message {
        data: MessageData::Assistant { content },
        timestamp,
        id: message_id.0.clone(),
        parent_message_id: parent_id,
    };

    state.conversation.add_message(message.clone());
    state.conversation.active_message_id = Some(message_id.0.clone());

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::MessageAdded {
            message,
            model: state.current_model.clone(),
        },
    });

    effects
}

fn handle_model_response_error(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    op_id: crate::app::domain::types::OpId,
    error: &str,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    if state.cancelled_ops.contains(&op_id) {
        return effects;
    }

    state.complete_operation();

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::Error {
            message: error.to_string(),
        },
    });

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::OperationCompleted { op_id },
    });

    effects
}

fn handle_cancel(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    target_op: Option<crate::app::domain::types::OpId>,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    let op = match &state.current_operation {
        Some(op) if target_op.map_or(true, |t| t == op.op_id) => op.clone(),
        _ => return effects,
    };

    state.record_cancelled_op(op.op_id);

    if let Some(pending) = state.pending_approval.take() {
        let tool_result = ToolResult::Error(ToolError::Cancelled(pending.tool_call.name.clone()));
        let parent_id = state.conversation.active_message_id.clone();

        let message = Message {
            data: MessageData::Tool {
                tool_use_id: pending.tool_call.id.clone(),
                result: tool_result,
            },
            timestamp: 0,
            id: format!("cancelled_{}", pending.tool_call.id),
            parent_message_id: parent_id,
        };
        state.conversation.add_message(message);
    }

    state.approval_queue.clear();
    state.active_streams.remove(&op.op_id);

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::OperationCancelled {
            op_id: op.op_id,
            info: CancellationInfo {
                pending_tool_calls: op.pending_tool_calls.len(),
            },
        },
    });

    effects.push(Effect::CancelOperation {
        session_id,
        op_id: op.op_id,
    });

    state.current_operation = None;

    effects
}

fn handle_hydrate(
    state: &mut AppState,
    _session_id: crate::app::domain::types::SessionId,
    events: Vec<SessionEvent>,
    starting_sequence: u64,
) -> Vec<Effect> {
    for event in events {
        apply_event_to_state(state, &event);
    }

    state.event_sequence = starting_sequence;

    vec![]
}

pub fn apply_event_to_state(state: &mut AppState, event: &SessionEvent) {
    match event {
        SessionEvent::MessageAdded { message, model } => {
            state.conversation.add_message(message.clone());
            state.current_model = model.clone();
            state.conversation.active_message_id = Some(message.id().to_string());
        }
        SessionEvent::ApprovalDecided {
            decision, remember, ..
        } => {
            if *decision == ApprovalDecision::Approved {
                if let Some(memory) = remember {
                    match memory {
                        ApprovalMemory::Tool(name) => {
                            state.approved_tools.insert(name.clone());
                        }
                        ApprovalMemory::BashPattern(pattern) => {
                            state.approved_bash_patterns.insert(pattern.clone());
                        }
                    }
                }
            }
            state.pending_approval = None;
        }
        SessionEvent::ModelChanged { model } => {
            state.current_model = model.clone();
        }
        SessionEvent::OperationCompleted { .. } => {
            state.current_operation = None;
        }
        SessionEvent::OperationCancelled { op_id, .. } => {
            state.record_cancelled_op(*op_id);
            state.current_operation = None;
        }
        _ => {}
    }

    state.event_sequence += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::domain::state::OperationState;
    use crate::app::domain::types::{
        MessageId, NonEmptyString, OpId, RequestId, SessionId, ToolCallId,
    };
    use crate::config::model::builtin;
    use std::collections::HashSet;

    fn test_state() -> AppState {
        AppState::new(SessionId::new(), builtin::claude_sonnet_4_20250514())
    }

    #[test]
    fn test_user_input_starts_operation() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let message_id = MessageId::new();

        let effects = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("Hello").unwrap(),
                op_id,
                message_id,
                timestamp: 1234567890,
            },
        );

        assert_eq!(state.conversation.messages.len(), 1);
        assert!(state.current_operation.is_some());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::CallModel { .. }))
        );
    }

    #[test]
    fn test_late_result_ignored_after_cancel() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let tool_call_id = ToolCallId::from_string("tc_1");

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: [tool_call_id.clone()].into_iter().collect(),
        });

        let _ = reduce(
            &mut state,
            Action::Cancel {
                session_id,
                op_id: None,
            },
        );

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        let effects = reduce(
            &mut state,
            Action::ToolResult {
                session_id,
                tool_call_id,
                tool_name: "test".to_string(),
                result: Ok(ToolResult::External(steer_tools::result::ExternalResult {
                    tool_name: "test".to_string(),
                    payload: "done".to_string(),
                })),
            },
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn test_pre_approved_tool_executes_immediately() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

        state.approved_tools.insert("test_tool".to_string());
        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        let tool_call = steer_tools::ToolCall {
            id: "tc_1".to_string(),
            name: "test_tool".to_string(),
            parameters: serde_json::json!({}),
        };

        let effects = reduce(
            &mut state,
            Action::ToolApprovalRequested {
                session_id,
                request_id: RequestId::new(),
                tool_call,
            },
        );

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExecuteTool { .. }))
        );
        assert!(state.pending_approval.is_none());
    }

    #[test]
    fn test_approval_queuing() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        let tool_call_1 = steer_tools::ToolCall {
            id: "tc_1".to_string(),
            name: "tool_1".to_string(),
            parameters: serde_json::json!({}),
        };
        let tool_call_2 = steer_tools::ToolCall {
            id: "tc_2".to_string(),
            name: "tool_2".to_string(),
            parameters: serde_json::json!({}),
        };

        let _ = reduce(
            &mut state,
            Action::ToolApprovalRequested {
                session_id,
                request_id: RequestId::new(),
                tool_call: tool_call_1,
            },
        );

        assert!(state.pending_approval.is_some());

        let _ = reduce(
            &mut state,
            Action::ToolApprovalRequested {
                session_id,
                request_id: RequestId::new(),
                tool_call: tool_call_2,
            },
        );

        assert_eq!(state.approval_queue.len(), 1);
    }
}
