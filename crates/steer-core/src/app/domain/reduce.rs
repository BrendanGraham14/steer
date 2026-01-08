use crate::app::conversation::{AssistantContent, Message, MessageData, UserContent};
use crate::app::domain::action::{Action, ApprovalDecision, ApprovalMemory, McpServerState};
use crate::app::domain::effect::{Effect, McpServerConfig};
use crate::app::domain::event::{CancellationInfo, SessionEvent};
use crate::app::domain::state::{AppState, OperationKind, PendingApproval, QueuedApproval};
use crate::session::state::BackendConfig;
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
            UserEditedMessageParams {
                original_message_id: message_id,
                new_content,
                op_id,
                new_message_id,
                model,
                timestamp,
            },
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
            message_id,
            command,
            timestamp,
        } => handle_direct_bash(state, session_id, op_id, message_id, command, timestamp),

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
            session_id,
            server_name,
            state: new_state,
        } => {
            // When connected, merge MCP tools into state.tools
            if let McpServerState::Connected { tools } = &new_state {
                let tools = state
                    .session_config
                    .as_ref()
                    .map(|config| config.filter_tools_by_visibility(tools.clone()))
                    .unwrap_or_else(|| tools.clone());

                // Add MCP tools that aren't already present (by name)
                for tool in tools {
                    if !state.tools.iter().any(|t| t.name == tool.name) {
                        state.tools.push(tool.clone());
                    }
                }
            }

            // When disconnected or failed, remove tools from that server
            if matches!(
                &new_state,
                McpServerState::Disconnected { .. } | McpServerState::Failed { .. }
            ) {
                let prefix = format!("mcp__{}__", server_name);
                state.tools.retain(|t| !t.name.starts_with(&prefix));
            }

            state
                .mcp_servers
                .insert(server_name.clone(), new_state.clone());
            vec![Effect::EmitEvent {
                session_id,
                event: SessionEvent::McpServerStateChanged {
                    server_name,
                    state: new_state,
                },
            }]
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
            CompactionCompleteParams {
                op_id,
                compaction_id,
                summary_message_id,
                summary,
                compacted_head_message_id,
                previous_active_message_id,
                model_name: model,
                timestamp,
            },
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

    let parent_id = state.message_graph.active_message_id.clone();

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

    state.message_graph.add_message(message.clone());
    state.message_graph.active_message_id = Some(message_id.0.clone());

    state.start_operation(op_id, OperationKind::AgentLoop);
    state.operation_models.insert(op_id, model.clone());

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::UserMessageAdded {
            message: message.clone(),
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
            .message_graph
            .get_thread_messages()
            .into_iter()
            .cloned()
            .collect(),
        system_prompt: state.cached_system_prompt.clone(),
        tools: state.tools.clone(),
    });

    effects
}

struct UserEditedMessageParams {
    original_message_id: crate::app::domain::types::MessageId,
    new_content: String,
    op_id: crate::app::domain::types::OpId,
    new_message_id: crate::app::domain::types::MessageId,
    model: crate::config::model::ModelId,
    timestamp: u64,
}

fn handle_user_edited_message(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    params: UserEditedMessageParams,
) -> Vec<Effect> {
    let UserEditedMessageParams {
        original_message_id,
        new_content,
        op_id,
        new_message_id,
        model,
        timestamp,
    } = params;
    let mut effects = Vec::new();

    let parent_id = state
        .message_graph
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

    state.message_graph.add_message(message.clone());
    state.message_graph.active_message_id = Some(new_message_id.0.clone());

    state.start_operation(op_id, OperationKind::AgentLoop);
    state.operation_models.insert(op_id, model.clone());

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::UserMessageAdded {
            message: message.clone(),
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
            .message_graph
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

    let is_direct_bash = matches!(
        state.current_operation.as_ref().map(|op| &op.kind),
        Some(OperationKind::DirectBash { .. })
    );

    if is_direct_bash {
        return vec![];
    }

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

    let op = match &state.current_operation {
        Some(op) => {
            if state.cancelled_ops.contains(&op.op_id) {
                tracing::debug!("Ignoring late tool result for cancelled op {:?}", op.op_id);
                return effects;
            }
            op.clone()
        }
        None => return effects,
    };
    let op_id = op.op_id;

    state.remove_pending_tool_call(&tool_call_id);

    let tool_result = match result {
        Ok(r) => r,
        Err(e) => ToolResult::Error(e),
    };

    let is_direct_bash = matches!(op.kind, OperationKind::DirectBash { .. });

    if is_direct_bash {
        let command = match &op.kind {
            OperationKind::DirectBash { command } => command.clone(),
            _ => tool_name,
        };

        let (stdout, stderr, exit_code) = match &tool_result {
            ToolResult::Bash(result) => (
                result.stdout.clone(),
                result.stderr.clone(),
                result.exit_code,
            ),
            ToolResult::Error(err) => (String::new(), err.to_string(), 1),
            other => (format!("{other:?}"), String::new(), 0),
        };

        let updated = state.operation_messages.remove(&op_id).and_then(|id| {
            state
                .message_graph
                .update_command_execution(
                    id.as_str(),
                    command.clone(),
                    stdout.clone(),
                    stderr.clone(),
                    exit_code,
                )
                .or_else(|| {
                    let parent_id = state.message_graph.active_message_id.clone();
                    let timestamp = Message::current_timestamp();
                    Some(Message {
                        data: MessageData::User {
                            content: vec![UserContent::CommandExecution {
                                command: command.clone(),
                                stdout: stdout.clone(),
                                stderr: stderr.clone(),
                                exit_code,
                            }],
                        },
                        timestamp,
                        id: id.to_string(),
                        parent_message_id: parent_id,
                    })
                })
        });

        state.complete_operation();

        if let Some(message) = updated {
            effects.push(Effect::EmitEvent {
                session_id,
                event: SessionEvent::MessageUpdated { message },
            });
        }

        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::OperationCompleted { op_id },
        });

        return effects;
    }

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

    let parent_id = state.message_graph.active_message_id.clone();
    let tool_message = Message {
        data: MessageData::Tool {
            tool_use_id: tool_call_id.0.clone(),
            result: tool_result,
        },
        timestamp: 0,
        id: format!("tool_result_{}", tool_call_id.0),
        parent_message_id: parent_id,
    };
    state.message_graph.add_message(tool_message.clone());

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::ToolMessageAdded {
            message: tool_message,
        },
    });

    let all_tools_complete = state
        .current_operation
        .as_ref()
        .map(|op| op.pending_tool_calls.is_empty())
        .unwrap_or(true);
    let no_pending_approvals = state.pending_approval.is_none() && state.approval_queue.is_empty();

    if all_tools_complete && no_pending_approvals {
        effects.push(Effect::CallModel {
            session_id,
            op_id,
            model,
            messages: state
                .message_graph
                .get_thread_messages()
                .into_iter()
                .cloned()
                .collect(),
            system_prompt: state.cached_system_prompt.clone(),
            tools: state.tools.clone(),
        });
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

    let parent_id = state.message_graph.active_message_id.clone();

    let message = Message {
        data: MessageData::Assistant {
            content: content.clone(),
        },
        timestamp,
        id: message_id.0.clone(),
        parent_message_id: parent_id,
    };

    state.message_graph.add_message(message.clone());
    state.message_graph.active_message_id = Some(message_id.0.clone());

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
        event: SessionEvent::AssistantMessageAdded { message, model },
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
    message_id: crate::app::domain::types::MessageId,
    command: String,
    timestamp: u64,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    let parent_id = state.message_graph.active_message_id.clone();
    let message = Message {
        data: MessageData::User {
            content: vec![UserContent::CommandExecution {
                command: command.clone(),
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            }],
        },
        timestamp,
        id: message_id.0.clone(),
        parent_message_id: parent_id,
    };

    state.message_graph.add_message(message.clone());
    state.message_graph.active_message_id = Some(message_id.0.clone());

    state.start_operation(
        op_id,
        OperationKind::DirectBash {
            command: command.clone(),
        },
    );
    state.operation_messages.insert(op_id, message_id);

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::UserMessageAdded { message },
    });

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
        id: format!("direct_bash_{op_id}"),
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
    let message_count = state.message_graph.get_thread_messages().len();

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
        Some(op) if target_op.is_none_or(|t| t == op.op_id) => op.clone(),
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
        let parent_id = state.message_graph.active_message_id.clone();

        let message = Message {
            data: MessageData::Tool {
                tool_use_id: pending.tool_call.id.clone(),
                result: tool_result,
            },
            timestamp: 0,
            id: format!("cancelled_{}", pending.tool_call.id),
            parent_message_id: parent_id,
        };
        state.message_graph.add_message(message);
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
    session_id: crate::app::domain::types::SessionId,
    events: Vec<SessionEvent>,
    starting_sequence: u64,
) -> Vec<Effect> {
    for event in events {
        apply_event_to_state(state, &event);
    }

    state.event_sequence = starting_sequence;

    emit_mcp_connect_effects(state, session_id)
}

pub fn apply_event_to_state(state: &mut AppState, event: &SessionEvent) {
    match event {
        SessionEvent::SessionCreated { config, .. } => {
            state.session_config = Some((**config).clone());
        }
        SessionEvent::AssistantMessageAdded { message, .. }
        | SessionEvent::UserMessageAdded { message }
        | SessionEvent::ToolMessageAdded { message } => {
            state.message_graph.add_message(message.clone());
            state.message_graph.active_message_id = Some(message.id().to_string());
        }
        SessionEvent::MessageUpdated { message } => {
            state.message_graph.replace_message(message.clone());
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
        SessionEvent::McpServerStateChanged {
            server_name,
            state: mcp_state,
        } => {
            state
                .mcp_servers
                .insert(server_name.clone(), mcp_state.clone());
        }
        _ => {}
    }

    state.event_sequence += 1;
}

struct CompactionCompleteParams {
    op_id: crate::app::domain::types::OpId,
    compaction_id: crate::app::domain::types::CompactionId,
    summary_message_id: crate::app::domain::types::MessageId,
    summary: String,
    compacted_head_message_id: crate::app::domain::types::MessageId,
    previous_active_message_id: Option<crate::app::domain::types::MessageId>,
    model_name: String,
    timestamp: u64,
}

fn handle_compaction_complete(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    params: CompactionCompleteParams,
) -> Vec<Effect> {
    use crate::app::conversation::{AssistantContent, Message, MessageData};
    use crate::app::domain::types::CompactionRecord;

    let CompactionCompleteParams {
        op_id,
        compaction_id,
        summary_message_id,
        summary,
        compacted_head_message_id,
        previous_active_message_id,
        model_name,
        timestamp,
    } = params;

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

    state.message_graph.add_message(summary_message.clone());

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
            event: SessionEvent::AssistantMessageAdded {
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

fn emit_mcp_connect_effects(
    state: &AppState,
    session_id: crate::app::domain::types::SessionId,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    let Some(ref config) = state.session_config else {
        return effects;
    };

    for backend_config in &config.tool_config.backends {
        let BackendConfig::Mcp {
            server_name,
            transport,
            tool_filter,
        } = backend_config;

        let already_connected = state.mcp_servers.get(server_name).is_some_and(|s| {
            matches!(
                s,
                McpServerState::Connecting | McpServerState::Connected { .. }
            )
        });

        if !already_connected {
            effects.push(Effect::ConnectMcpServer {
                session_id,
                config: McpServerConfig {
                    server_name: server_name.clone(),
                    transport: transport.clone(),
                    tool_filter: tool_filter.clone(),
                },
            });
        }
    }

    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::domain::state::OperationState;
    use crate::app::domain::types::{
        MessageId, NonEmptyString, OpId, RequestId, SessionId, ToolCallId,
    };
    use crate::config::model::builtin;
    use crate::session::state::{SessionConfig, ToolVisibility};
    use std::collections::HashSet;
    use steer_tools::{InputSchema, ToolSchema};

    fn test_state() -> AppState {
        AppState::new(SessionId::new())
    }

    fn test_schema(name: &str) -> ToolSchema {
        ToolSchema {
            name: name.to_string(),
            description: String::new(),
            input_schema: InputSchema {
                properties: Default::default(),
                required: Vec::new(),
                schema_type: "object".to_string(),
            },
        }
    }

    #[test]
    fn test_user_input_starts_operation() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let message_id = MessageId::new();
        let model = builtin::claude_sonnet_4_5();

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

        assert_eq!(state.message_graph.messages.len(), 1);
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
            .insert(op_id, builtin::claude_sonnet_4_5());
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());

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
            .insert(op_id, builtin::claude_sonnet_4_5());
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());

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
            .insert(op_id, builtin::claude_sonnet_4_5());

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
            .insert(op_id, builtin::claude_sonnet_4_5());

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
    fn test_mcp_tool_visibility_and_disconnect_removal() {
        let mut state = test_state();
        let session_id = state.session_id;

        let mut allowed = HashSet::new();
        allowed.insert("mcp__alpha__allowed".to_string());

        let mut config = SessionConfig::read_only();
        config.tool_config.visibility = ToolVisibility::Whitelist(allowed);
        state.session_config = Some(config);

        state.tools.push(test_schema("bash"));

        let _ = reduce(
            &mut state,
            Action::McpServerStateChanged {
                session_id,
                server_name: "alpha".to_string(),
                state: McpServerState::Connected {
                    tools: vec![
                        test_schema("mcp__alpha__allowed"),
                        test_schema("mcp__alpha__blocked"),
                    ],
                },
            },
        );

        assert!(state.tools.iter().any(|t| t.name == "mcp__alpha__allowed"));
        assert!(!state.tools.iter().any(|t| t.name == "mcp__alpha__blocked"));

        let _ = reduce(
            &mut state,
            Action::McpServerStateChanged {
                session_id,
                server_name: "alpha".to_string(),
                state: McpServerState::Disconnected { error: None },
            },
        );

        assert!(
            !state
                .tools
                .iter()
                .any(|t| t.name.starts_with("mcp__alpha__"))
        );
        assert!(state.tools.iter().any(|t| t.name == "bash"));
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
            .insert(op_id, builtin::claude_sonnet_4_5());

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
            .insert(op_id, builtin::claude_sonnet_4_5());

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
