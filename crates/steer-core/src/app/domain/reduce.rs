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
            model,
            timestamp,
        } => handle_user_input(state, session_id, text, op_id, message_id, model, timestamp),

        Action::UserEditedMessage {
            session_id,
            message_id,
            new_content,
            op_id,
            new_message_id,
            model,
            timestamp,
        } => handle_user_edited_message(
            state,
            session_id,
            message_id,
            new_content,
            op_id,
            new_message_id,
            model,
            timestamp,
        ),

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

        Action::DirectBashCommand {
            session_id,
            op_id,
            command,
            model,
        } => handle_direct_bash(state, session_id, op_id, command, model),

        Action::RequestCompaction {
            session_id,
            op_id,
            model,
        } => handle_request_compaction(state, session_id, op_id, model),

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

        Action::CompactionComplete {
            session_id,
            op_id,
            compaction_id,
            summary_message_id,
            summary,
            compacted_head_message_id,
            previous_active_message_id,
            model,
            timestamp,
        } => handle_compaction_complete(
            state,
            session_id,
            op_id,
            compaction_id,
            summary_message_id,
            summary,
            compacted_head_message_id,
            previous_active_message_id,
            model,
            timestamp,
        ),

        Action::CompactionFailed {
            session_id,
            op_id,
            error,
        } => handle_compaction_failed(state, session_id, op_id, error),

        Action::Shutdown => vec![],
    }
}

fn handle_user_input(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    text: crate::app::domain::types::NonEmptyString,
    op_id: crate::app::domain::types::OpId,
    message_id: crate::app::domain::types::MessageId,
    model: crate::config::model::ModelId,
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
    state.operation_models.insert(op_id, model.clone());

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::MessageAdded {
            message: message.clone(),
            model: model.clone(),
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
        model,
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
    model: crate::config::model::ModelId,
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
    state.operation_models.insert(op_id, model.clone());

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::MessageAdded {
            message: message.clone(),
            model: model.clone(),
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
        model,
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

    let resolved_memory = if decision == ApprovalDecision::Approved {
        match remember {
            Some(ApprovalMemory::PendingTool) => {
                Some(ApprovalMemory::Tool(pending.tool_call.name.clone()))
            }
            Some(ApprovalMemory::Tool(name)) => Some(ApprovalMemory::Tool(name)),
            Some(ApprovalMemory::BashPattern(pattern)) => {
                Some(ApprovalMemory::BashPattern(pattern))
            }
            None => None,
        }
    } else {
        None
    };

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::ApprovalDecided {
            request_id,
            decision,
            remember: resolved_memory.clone(),
        },
    });

    if decision == ApprovalDecision::Approved {
        if let Some(ref memory) = resolved_memory {
            match memory {
                ApprovalMemory::Tool(name) => {
                    state.approved_tools.insert(name.clone());
                }
                ApprovalMemory::BashPattern(pattern) => {
                    state.approved_bash_patterns.insert(pattern.clone());
                }
                ApprovalMemory::PendingTool => {}
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
        )
    {
        return state.is_bash_pattern_approved(&params.command);
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

    let op_id = match state.current_operation.as_ref() {
        Some(op) => op.op_id,
        None => {
            return vec![Effect::EmitEvent {
                session_id,
                event: SessionEvent::Error {
                    message: "Tool call started without active operation".to_string(),
                },
            }];
        }
    };

    let model = match state.operation_models.get(&op_id).cloned() {
        Some(model) => model,
        None => {
            return vec![Effect::EmitEvent {
                session_id,
                event: SessionEvent::Error {
                    message: format!("Missing model for tool call on operation {op_id}"),
                },
            }];
        }
    };

    vec![Effect::EmitEvent {
        session_id,
        event: SessionEvent::ToolCallStarted {
            id: tool_call_id,
            name: tool_name,
            parameters: tool_parameters,
            model,
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

    let op_id = match &state.current_operation {
        Some(op) => {
            if state.cancelled_ops.contains(&op.op_id) {
                tracing::debug!("Ignoring late tool result for cancelled op {:?}", op.op_id);
                return effects;
            }
            op.op_id
        }
        None => return effects,
    };

    state.remove_pending_tool_call(&tool_call_id);

    let tool_result = match result {
        Ok(r) => r,
        Err(e) => ToolResult::Error(e),
    };

    let model = match state.operation_models.get(&op_id).cloned() {
        Some(model) => model,
        None => {
            return vec![Effect::EmitEvent {
                session_id,
                event: SessionEvent::Error {
                    message: format!("Missing model for tool result on operation {op_id}"),
                },
            }];
        }
    };

    let event = match &tool_result {
        ToolResult::Error(e) => SessionEvent::ToolCallFailed {
            id: tool_call_id.clone(),
            name: tool_name.clone(),
            error: e.to_string(),
            model: model.clone(),
        },
        _ => SessionEvent::ToolCallCompleted {
            id: tool_call_id.clone(),
            name: tool_name,
            result: tool_result.clone(),
            model: model.clone(),
        },
    };

    effects.push(Effect::EmitEvent { session_id, event });

    let parent_id = state.conversation.active_message_id.clone();
    let tool_message = Message {
        data: MessageData::Tool {
            tool_use_id: tool_call_id.0.clone(),
            result: tool_result,
        },
        timestamp: 0,
        id: format!("tool_result_{}", tool_call_id.0),
        parent_message_id: parent_id,
    };
    state.conversation.add_message(tool_message.clone());

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::MessageAdded {
            message: tool_message,
            model: model.clone(),
        },
    });

    let all_tools_complete = state
        .current_operation
        .as_ref()
        .map(|op| op.pending_tool_calls.is_empty())
        .unwrap_or(true);
    let no_pending_approvals = state.pending_approval.is_none() && state.approval_queue.is_empty();

    if all_tools_complete && no_pending_approvals {
        let is_direct_bash = matches!(
            state.current_operation.as_ref().map(|op| &op.kind),
            Some(OperationKind::DirectBash { .. })
        );

        if is_direct_bash {
            state.complete_operation();
            effects.push(Effect::EmitEvent {
                session_id,
                event: SessionEvent::OperationCompleted { op_id },
            });
        } else {
            effects.push(Effect::CallModel {
                session_id,
                op_id,
                model,
                messages: state
                    .conversation
                    .get_thread_messages()
                    .into_iter()
                    .cloned()
                    .collect(),
                system_prompt: state.cached_system_prompt.clone(),
                tools: state.tools.clone(),
            });
        }
    }

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

    let tool_calls: Vec<_> = content
        .iter()
        .filter_map(|c| {
            if let AssistantContent::ToolCall { tool_call } = c {
                Some(tool_call.clone())
            } else {
                None
            }
        })
        .collect();

    let parent_id = state.conversation.active_message_id.clone();

    let message = Message {
        data: MessageData::Assistant {
            content: content.clone(),
        },
        timestamp,
        id: message_id.0.clone(),
        parent_message_id: parent_id,
    };

    state.conversation.add_message(message.clone());
    state.conversation.active_message_id = Some(message_id.0.clone());

    let model = match state.operation_models.get(&op_id).cloned() {
        Some(model) => model,
        None => {
            return vec![Effect::EmitEvent {
                session_id,
                event: SessionEvent::Error {
                    message: format!("Missing model for operation {op_id}"),
                },
            }];
        }
    };

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::MessageAdded { message, model },
    });

    if tool_calls.is_empty() {
        state.complete_operation();
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::OperationCompleted { op_id },
        });
    } else {
        for tool_call in tool_calls {
            let request_id = crate::app::domain::types::RequestId::new();
            effects.extend(handle_tool_approval_requested(
                state, session_id, request_id, tool_call,
            ));
        }
    }

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

fn handle_direct_bash(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    op_id: crate::app::domain::types::OpId,
    command: String,
    model: crate::config::model::ModelId,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    state.start_operation(
        op_id,
        OperationKind::DirectBash {
            command: command.clone(),
        },
    );
    state.operation_models.insert(op_id, model);

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::OperationStarted {
            op_id,
            kind: OperationKind::DirectBash {
                command: command.clone(),
            },
        },
    });

    let tool_call = steer_tools::ToolCall {
        id: format!("direct_bash_{}", op_id),
        name: BASH_TOOL_NAME.to_string(),
        parameters: serde_json::json!({ "command": command }),
    };

    effects.push(Effect::ExecuteTool {
        session_id,
        op_id,
        tool_call,
    });

    effects
}

fn handle_request_compaction(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    op_id: crate::app::domain::types::OpId,
    model: crate::config::model::ModelId,
) -> Vec<Effect> {
    const MIN_MESSAGES_FOR_COMPACT: usize = 3;
    let message_count = state.conversation.get_thread_messages().len();

    if message_count < MIN_MESSAGES_FOR_COMPACT {
        return vec![Effect::EmitEvent {
            session_id,
            event: SessionEvent::CompactResult {
                result: crate::app::domain::event::CompactResult::InsufficientMessages,
            },
        }];
    }

    state.start_operation(op_id, OperationKind::Compact);
    state.operation_models.insert(op_id, model.clone());

    vec![
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::OperationStarted {
                op_id,
                kind: OperationKind::Compact,
            },
        },
        Effect::RequestCompaction {
            session_id,
            op_id,
            model,
        },
    ]
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

    if matches!(op.kind, OperationKind::Compact) {
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::CompactResult {
                result: crate::app::domain::event::CompactResult::Cancelled,
            },
        });
    }

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

    state.complete_operation();

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
        SessionEvent::SessionCreated { config, .. } => {
            state.session_config = Some(config.clone());
        }
        SessionEvent::MessageAdded { message, .. } => {
            state.conversation.add_message(message.clone());
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
                        ApprovalMemory::PendingTool => {}
                    }
                }
            }
            state.pending_approval = None;
        }
        SessionEvent::OperationCompleted { .. } => {
            state.complete_operation();
        }
        SessionEvent::OperationCancelled { op_id, .. } => {
            state.record_cancelled_op(*op_id);
            state.complete_operation();
        }
        _ => {}
    }

    state.event_sequence += 1;
}

fn handle_compaction_complete(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    op_id: crate::app::domain::types::OpId,
    compaction_id: crate::app::domain::types::CompactionId,
    summary_message_id: crate::app::domain::types::MessageId,
    summary: String,
    compacted_head_message_id: crate::app::domain::types::MessageId,
    previous_active_message_id: Option<crate::app::domain::types::MessageId>,
    model_name: String,
    timestamp: u64,
) -> Vec<Effect> {
    use crate::app::conversation::{AssistantContent, Message, MessageData};
    use crate::app::domain::types::CompactionRecord;

    let summary_message = Message {
        data: MessageData::Assistant {
            content: vec![AssistantContent::Text {
                text: summary.clone(),
            }],
        },
        id: summary_message_id.to_string(),
        parent_message_id: None,
        timestamp,
    };

    state.conversation.add_message(summary_message.clone());

    let record = CompactionRecord::with_timestamp(
        compaction_id,
        summary_message_id,
        compacted_head_message_id,
        previous_active_message_id,
        model_name,
        timestamp,
    );

    let model = match state.operation_models.get(&op_id).cloned() {
        Some(model) => model,
        None => {
            state.complete_operation();
            return vec![Effect::EmitEvent {
                session_id,
                event: SessionEvent::Error {
                    message: format!("Missing model for compaction operation {op_id}"),
                },
            }];
        }
    };

    state.complete_operation();

    vec![
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::MessageAdded {
                message: summary_message,
                model,
            },
        },
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::CompactResult {
                result: crate::app::domain::event::CompactResult::Success(summary),
            },
        },
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::ConversationCompacted { record },
        },
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::OperationCompleted { op_id },
        },
    ]
}

fn handle_compaction_failed(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    op_id: crate::app::domain::types::OpId,
    error: String,
) -> Vec<Effect> {
    state.complete_operation();

    vec![
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::Error { message: error },
        },
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::OperationCompleted { op_id },
        },
    ]
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
        AppState::new(SessionId::new())
    }

    #[test]
    fn test_user_input_starts_operation() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let message_id = MessageId::new();
        let model = builtin::claude_sonnet_4_20250514();

        let effects = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("Hello").unwrap(),
                op_id,
                message_id,
                model,
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
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());

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
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());

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

    #[test]
    fn test_model_response_with_tool_calls_requests_approval() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let message_id = MessageId::new();

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());

        let tool_call = steer_tools::ToolCall {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            parameters: serde_json::json!({"command": "ls"}),
        };

        let content = vec![
            AssistantContent::Text {
                text: "Let me list the files.".to_string(),
            },
            AssistantContent::ToolCall {
                tool_call: tool_call.clone(),
            },
        ];

        let effects = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id,
                content,
                timestamp: 12345,
            },
        );

        assert!(state.pending_approval.is_some());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RequestUserApproval { .. }))
        );
        assert!(state.current_operation.is_some());
    }

    #[test]
    fn test_model_response_no_tools_completes_operation() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let message_id = MessageId::new();

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());

        let content = vec![AssistantContent::Text {
            text: "Hello! How can I help?".to_string(),
        }];

        let effects = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id,
                content,
                timestamp: 12345,
            },
        );

        assert!(state.current_operation.is_none());
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::OperationCompleted { .. },
                ..
            }
        )));
    }

    #[test]
    fn test_tool_result_continues_agent_loop() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let tool_call_id = ToolCallId::from_string("tc_1");

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: [tool_call_id.clone()].into_iter().collect(),
        });
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());

        let effects = reduce(
            &mut state,
            Action::ToolResult {
                session_id,
                tool_call_id,
                tool_name: "bash".to_string(),
                result: Ok(ToolResult::External(steer_tools::result::ExternalResult {
                    tool_name: "bash".to_string(),
                    payload: "file1.txt\nfile2.txt".to_string(),
                })),
            },
        );

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::CallModel { .. }))
        );
    }

    #[test]
    fn test_tool_result_waits_for_pending_tools() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let tool_call_id_1 = ToolCallId::from_string("tc_1");
        let tool_call_id_2 = ToolCallId::from_string("tc_2");

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: [tool_call_id_1.clone(), tool_call_id_2.clone()]
                .into_iter()
                .collect(),
        });
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_20250514());

        let effects = reduce(
            &mut state,
            Action::ToolResult {
                session_id,
                tool_call_id: tool_call_id_1,
                tool_name: "bash".to_string(),
                result: Ok(ToolResult::External(steer_tools::result::ExternalResult {
                    tool_name: "bash".to_string(),
                    payload: "done".to_string(),
                })),
            },
        );

        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::CallModel { .. }))
        );

        let effects = reduce(
            &mut state,
            Action::ToolResult {
                session_id,
                tool_call_id: tool_call_id_2,
                tool_name: "bash".to_string(),
                result: Ok(ToolResult::External(steer_tools::result::ExternalResult {
                    tool_name: "bash".to_string(),
                    payload: "done".to_string(),
                })),
            },
        );

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::CallModel { .. }))
        );
    }
}
