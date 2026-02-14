use crate::agents::default_agent_spec_id;
use crate::api::provider::TokenUsage;
use crate::app::conversation::{AssistantContent, Message, MessageData, UserContent};

use crate::app::domain::action::{Action, ApprovalDecision, ApprovalMemory, McpServerState};

use crate::app::domain::effect::{Effect, McpServerConfig};
use crate::app::domain::event::{
    CancellationInfo, ContextWindowUsage, QueuedWorkItemSnapshot, QueuedWorkKind, SessionEvent,
};
use crate::app::domain::state::{
    AppState, OperationKind, PendingApproval, QueuedApproval, QueuedWorkItem,
};
use crate::primary_agents::{
    default_primary_agent_id, primary_agent_spec, resolve_effective_config,
};
use crate::session::state::{BackendConfig, ToolDecision};

use crate::app::domain::event::CompactTrigger;
use crate::tools::{DISPATCH_AGENT_TOOL_NAME, DispatchAgentParams, DispatchAgentTarget};
use serde_json::Value;
use steer_tools::ToolError;
use steer_tools::result::ToolResult;
use steer_tools::tools::BASH_TOOL_NAME;
use thiserror::Error;

const MIN_MESSAGES_FOR_COMPACT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidActionKind {
    OperationInFlight,
    MissingSessionConfig,
    UnknownPrimaryAgent,
    QueueEmpty,
}

#[derive(Debug, Error)]
pub enum ReduceError {
    #[error("{message}")]
    InvalidAction {
        message: String,
        kind: InvalidActionKind,
    },
    #[error("Invariant violated: {message}")]
    Invariant { message: String },
}

fn invalid_action(kind: InvalidActionKind, message: impl Into<String>) -> ReduceError {
    ReduceError::InvalidAction {
        message: message.into(),
        kind,
    }
}

fn derive_context_window_usage(
    total_tokens: u32,
    context_window_tokens: Option<u32>,
) -> Option<ContextWindowUsage> {
    context_window_tokens.map(|max_context_tokens| {
        let remaining_tokens = max_context_tokens.saturating_sub(total_tokens);
        let (utilization_ratio, estimated) = if max_context_tokens > 0 {
            let ratio = (f64::from(total_tokens) / f64::from(max_context_tokens)).clamp(0.0, 1.0);
            (Some(ratio), false)
        } else {
            (None, true)
        };

        ContextWindowUsage {
            max_context_tokens: Some(max_context_tokens),
            remaining_tokens: Some(remaining_tokens),
            utilization_ratio,
            estimated,
        }
    })
}

pub fn reduce(state: &mut AppState, action: Action) -> Result<Vec<Effect>, ReduceError> {
    match action {
        Action::UserInput {
            session_id,
            content,
            op_id,
            message_id,
            model,
            timestamp,
        } => Ok(handle_user_input(
            state, session_id, content, op_id, message_id, model, timestamp,
        )),

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
        } => Ok(handle_tool_approval_requested(
            state, session_id, request_id, tool_call,
        )),

        Action::ToolApprovalDecided {
            session_id,
            request_id,
            decision,
            remember,
        } => Ok(handle_tool_approval_decided(
            state, session_id, request_id, decision, remember,
        )),

        Action::ToolExecutionStarted {
            session_id,
            tool_call_id,
            tool_name,
            tool_parameters,
        } => Ok(handle_tool_execution_started(
            state,
            session_id,
            tool_call_id,
            tool_name,
            tool_parameters,
        )),

        Action::ToolResult {
            session_id,
            tool_call_id,
            tool_name,
            result,
        } => Ok(handle_tool_result(
            state,
            session_id,
            tool_call_id,
            tool_name,
            result,
        )),

        Action::ModelResponseComplete {
            session_id,
            op_id,
            message_id,
            content,
            usage,
            context_window_tokens,
            timestamp,
        } => Ok(handle_model_response_complete(
            state,
            session_id,
            ModelResponseCompleteParams {
                op_id,
                message_id,
                content,
                usage,
                context_window_tokens,
                timestamp,
            },
        )),

        Action::ModelResponseError {
            session_id,
            op_id,
            error,
        } => Ok(handle_model_response_error(
            state, session_id, op_id, &error,
        )),

        Action::Cancel { session_id, op_id } => Ok(handle_cancel(state, session_id, op_id)),

        Action::DirectBashCommand {
            session_id,
            op_id,
            message_id,
            command,
            timestamp,
        } => Ok(handle_direct_bash(
            state, session_id, op_id, message_id, command, timestamp,
        )),

        Action::DequeueQueuedItem { session_id } => handle_dequeue_queued_item(state, session_id),

        Action::DrainQueuedWork { session_id } => Ok(maybe_start_queued_work(state, session_id)),

        Action::RequestCompaction {
            session_id,
            op_id,
            model,
        } => handle_request_compaction(state, session_id, op_id, model),

        Action::Hydrate {
            session_id,
            events,
            starting_sequence,
        } => Ok(handle_hydrate(state, session_id, events, starting_sequence)),

        Action::WorkspaceFilesListed { files, .. } => {
            state.workspace_files = files;
            Ok(vec![])
        }

        Action::ToolSchemasAvailable { tools, .. } => {
            state.tools = tools;
            Ok(vec![])
        }

        Action::ToolSchemasUpdated { schemas, .. } => {
            state.tools = schemas;
            Ok(vec![])
        }

        Action::SwitchPrimaryAgent {
            session_id,
            agent_id,
        } => handle_switch_primary_agent(state, session_id, agent_id),

        Action::McpServerStateChanged {
            session_id,
            server_name,
            state: new_state,
        } => {
            // When connected, merge MCP tools into state.tools
            if let McpServerState::Connected { tools } = &new_state {
                let tools = state.session_config.as_ref().map_or_else(
                    || tools.clone(),
                    |config| config.filter_tools_by_visibility(tools.clone()),
                );

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
                let prefix = format!("mcp__{server_name}__");
                state.tools.retain(|t| !t.name.starts_with(&prefix));
            }

            state
                .mcp_servers
                .insert(server_name.clone(), new_state.clone());
            Ok(vec![Effect::EmitEvent {
                session_id,
                event: SessionEvent::McpServerStateChanged {
                    server_name,
                    state: new_state,
                },
            }])
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
        } => Ok(handle_compaction_complete(
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
        )),

        Action::CompactionFailed {
            session_id,
            op_id,
            error,
        } => Ok(handle_compaction_failed(state, session_id, op_id, error)),

        Action::Shutdown => Ok(vec![]),
    }
}

fn handle_user_input(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    content: Vec<UserContent>,
    op_id: crate::app::domain::types::OpId,
    message_id: crate::app::domain::types::MessageId,
    model: crate::config::model::ModelId,
    timestamp: u64,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    if state.has_active_operation() {
        state.queue_user_message(crate::app::domain::state::QueuedUserMessage {
            content,
            op_id,
            message_id,
            model,
            queued_at: timestamp,
        });
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::QueueUpdated {
                queue: snapshot_queue(state),
            },
        });
        return effects;
    }

    let parent_id = state.message_graph.active_message_id.clone();

    let message = Message {
        data: MessageData::User { content },
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
        system_context: state.cached_system_context.clone(),
        tools: state.tools.clone(),
    });

    effects
}

struct UserEditedMessageParams {
    original_message_id: crate::app::domain::types::MessageId,
    new_content: Vec<UserContent>,
    op_id: crate::app::domain::types::OpId,
    new_message_id: crate::app::domain::types::MessageId,
    model: crate::config::model::ModelId,
    timestamp: u64,
}

fn handle_user_edited_message(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    params: UserEditedMessageParams,
) -> Result<Vec<Effect>, ReduceError> {
    let UserEditedMessageParams {
        original_message_id,
        new_content,
        op_id,
        new_message_id,
        model,
        timestamp,
    } = params;
    let mut effects = Vec::new();

    if state.has_active_operation() {
        return Err(invalid_action(
            InvalidActionKind::OperationInFlight,
            "Cannot edit message while an operation is active.",
        ));
    }

    let parent_id = state
        .message_graph
        .messages
        .iter()
        .find(|m| m.id() == original_message_id.0)
        .and_then(|m| m.parent_message_id().map(|s| s.to_string()));

    let message = Message {
        data: MessageData::User {
            content: new_content,
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
        system_context: state.cached_system_context.clone(),
        tools: state.tools.clone(),
    });

    Ok(effects)
}

fn handle_tool_approval_requested(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    request_id: crate::app::domain::types::RequestId,
    tool_call: steer_tools::ToolCall,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    if let Err(error) = validate_tool_call(state, &tool_call) {
        let error_message = error.to_string();
        return fail_tool_call_without_execution(
            state,
            session_id,
            tool_call,
            error,
            error_message,
            "invalid",
            true,
        );
    }

    let decision = get_tool_decision(state, &tool_call);

    match decision {
        ToolDecision::Allow => {
            let Some(op_id) = state.current_operation.as_ref().map(|o| o.op_id) else {
                return vec![Effect::EmitEvent {
                    session_id,
                    event: SessionEvent::Error {
                        message: "Tool approval requested without active operation".to_string(),
                    },
                }];
            };
            state.add_pending_tool_call(crate::app::domain::types::ToolCallId::from_string(
                &tool_call.id,
            ));

            effects.push(Effect::ExecuteTool {
                session_id,
                op_id,
                tool_call,
            });
        }
        ToolDecision::Deny => {
            let error = ToolError::DeniedByPolicy(tool_call.name.clone());
            let tool_name = tool_call.name.clone();
            effects.extend(fail_tool_call_without_execution(
                state,
                session_id,
                tool_call,
                error,
                format!("Tool '{tool_name}' denied by policy"),
                "denied",
                true,
            ));
        }
        ToolDecision::Ask => {
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
        }
    }

    effects
}

fn validate_tool_call(
    state: &AppState,
    tool_call: &steer_tools::ToolCall,
) -> Result<(), ToolError> {
    if tool_call.name.trim().is_empty() {
        return Err(ToolError::invalid_params(
            "unknown",
            "Malformed tool call: missing tool name",
        ));
    }

    if tool_call.id.trim().is_empty() {
        return Err(ToolError::invalid_params(
            tool_call.name.clone(),
            "Malformed tool call: missing tool call id",
        ));
    }

    if state.tools.is_empty() {
        return Ok(());
    }

    let Some(schema) = state.tools.iter().find(|s| s.name == tool_call.name) else {
        return Ok(());
    };

    validate_against_json_schema(
        &tool_call.name,
        schema.input_schema.as_value(),
        &tool_call.parameters,
    )
}

fn validate_against_json_schema(
    tool_name: &str,
    schema: &Value,
    params: &Value,
) -> Result<(), ToolError> {
    let validator = jsonschema::JSONSchema::compile(schema).map_err(|e| {
        ToolError::InternalError(format!("Invalid schema for tool '{tool_name}': {e}"))
    })?;

    if let Err(errors) = validator.validate(params) {
        let message = errors
            .into_iter()
            .map(|error| error.to_string())
            .next()
            .unwrap_or_else(|| "Parameters do not match schema".to_string());
        return Err(ToolError::invalid_params(tool_name.to_string(), message));
    }

    Ok(())
}

fn emit_tool_failure_message(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    tool_call_id: &str,
    tool_name: &str,
    tool_error: ToolError,
    event_error: String,
    message_id_prefix: &str,
) -> Vec<Effect> {
    let mut effects = Vec::new();

    let tool_result = ToolResult::Error(tool_error);
    let parent_id = state.message_graph.active_message_id.clone();
    let tool_message = Message {
        data: MessageData::Tool {
            tool_use_id: tool_call_id.to_string(),
            result: tool_result,
        },
        timestamp: 0,
        id: format!("{message_id_prefix}_{tool_call_id}"),
        parent_message_id: parent_id,
    };
    state.message_graph.add_message(tool_message.clone());

    if let Some(model) = state
        .current_operation
        .as_ref()
        .and_then(|op| state.operation_models.get(&op.op_id).cloned())
    {
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::ToolCallFailed {
                id: crate::app::domain::types::ToolCallId::from_string(tool_call_id),
                name: tool_name.to_string(),
                error: event_error,
                model,
            },
        });
    }

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::ToolMessageAdded {
            message: tool_message,
        },
    });

    effects
}

fn fail_tool_call_without_execution(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    tool_call: steer_tools::ToolCall,
    tool_error: ToolError,
    event_error: String,
    message_id_prefix: &str,
    call_model_if_ready: bool,
) -> Vec<Effect> {
    let mut effects = emit_tool_failure_message(
        state,
        session_id,
        &tool_call.id,
        &tool_call.name,
        tool_error,
        event_error,
        message_id_prefix,
    );

    if !call_model_if_ready {
        return effects;
    }

    let Some(op_id) = state.current_operation.as_ref().map(|o| o.op_id) else {
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::Error {
                message: "Tool failure recorded without active operation".to_string(),
            },
        });
        return effects;
    };
    let model = if let Some(model) = state.operation_models.get(&op_id).cloned() {
        model
    } else {
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::Error {
                message: format!("Missing model for operation {op_id}"),
            },
        });
        return effects;
    };

    let all_tools_complete = state
        .current_operation
        .as_ref()
        .is_none_or(|op| op.pending_tool_calls.is_empty());
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
            system_context: state.cached_system_context.clone(),
            tools: state.tools.clone(),
        });
    }

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

        let Some(op_id) = state.current_operation.as_ref().map(|o| o.op_id) else {
            effects.push(Effect::EmitEvent {
                session_id,
                event: SessionEvent::Error {
                    message: "Tool approval decided without active operation".to_string(),
                },
            });
            return effects;
        };
        state.add_pending_tool_call(crate::app::domain::types::ToolCallId::from_string(
            &pending.tool_call.id,
        ));

        effects.push(Effect::ExecuteTool {
            session_id,
            op_id,
            tool_call: pending.tool_call,
        });
    } else {
        let tool_name = pending.tool_call.name.clone();
        let error = ToolError::DeniedByUser(tool_name.clone());
        effects.extend(fail_tool_call_without_execution(
            state,
            session_id,
            pending.tool_call,
            error,
            format!("Tool '{tool_name}' denied by user"),
            "denied",
            false,
        ));
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
        let decision = get_tool_decision(state, &queued.tool_call);

        match decision {
            ToolDecision::Allow => {
                let Some(op_id) = state.current_operation.as_ref().map(|o| o.op_id) else {
                    effects.push(Effect::EmitEvent {
                        session_id,
                        event: SessionEvent::Error {
                            message: "Queued tool approval processed without active operation"
                                .to_string(),
                        },
                    });
                    state.approval_queue.push_front(queued);
                    break;
                };
                state.add_pending_tool_call(crate::app::domain::types::ToolCallId::from_string(
                    &queued.tool_call.id,
                ));

                effects.push(Effect::ExecuteTool {
                    session_id,
                    op_id,
                    tool_call: queued.tool_call,
                });
            }
            ToolDecision::Deny => {
                let tool_name = queued.tool_call.name.clone();
                let error = ToolError::DeniedByPolicy(tool_name.clone());
                effects.extend(fail_tool_call_without_execution(
                    state,
                    session_id,
                    queued.tool_call,
                    error,
                    format!("Tool '{tool_name}' denied by policy"),
                    "denied",
                    false,
                ));
            }
            ToolDecision::Ask => {
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
        }
    }

    let all_tools_complete = state
        .current_operation
        .as_ref()
        .is_none_or(|op| op.pending_tool_calls.is_empty());
    let no_pending_approvals = state.pending_approval.is_none() && state.approval_queue.is_empty();

    if all_tools_complete
        && no_pending_approvals
        && let Some(op) = &state.current_operation
    {
        let op_id = op.op_id;
        if let Some(model) = state.operation_models.get(&op_id).cloned() {
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
                system_context: state.cached_system_context.clone(),
                tools: state.tools.clone(),
            });
        }
    }

    effects
}

fn get_tool_decision(state: &AppState, tool_call: &steer_tools::ToolCall) -> ToolDecision {
    if state.approved_tools.contains(&tool_call.name) {
        return ToolDecision::Allow;
    }

    if tool_call.name == DISPATCH_AGENT_TOOL_NAME
        && let Ok(params) =
            serde_json::from_value::<DispatchAgentParams>(tool_call.parameters.clone())
        && let Some(config) = state.session_config.as_ref()
    {
        let policy = &config.tool_config.approval_policy;
        match params.target {
            DispatchAgentTarget::Resume { .. } => {
                return ToolDecision::Allow;
            }
            DispatchAgentTarget::New { agent, .. } => {
                let agent_id = agent
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map_or_else(|| default_agent_spec_id().to_string(), str::to_string);
                if policy.is_dispatch_agent_pattern_preapproved(&agent_id) {
                    return ToolDecision::Allow;
                }
            }
        }
    }

    if tool_call.name == BASH_TOOL_NAME
        && let Ok(params) = serde_json::from_value::<steer_tools::tools::bash::BashParams>(
            tool_call.parameters.clone(),
        )
        && state.is_bash_pattern_approved(&params.command)
    {
        return ToolDecision::Allow;
    }

    state
        .session_config
        .as_ref()
        .map_or(ToolDecision::Ask, |config| {
            config
                .tool_config
                .approval_policy
                .tool_decision(&tool_call.name)
        })
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

        state.complete_operation(op_id);

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

        effects.extend(maybe_start_queued_work(state, session_id));

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
        .is_none_or(|op| op.pending_tool_calls.is_empty());
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
            system_context: state.cached_system_context.clone(),
            tools: state.tools.clone(),
        });
    }

    effects
}

struct ModelResponseCompleteParams {
    op_id: crate::app::domain::types::OpId,
    message_id: crate::app::domain::types::MessageId,
    content: Vec<AssistantContent>,
    usage: Option<TokenUsage>,
    context_window_tokens: Option<u32>,
    timestamp: u64,
}

fn handle_model_response_complete(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    params: ModelResponseCompleteParams,
) -> Vec<Effect> {
    let ModelResponseCompleteParams {
        op_id,
        message_id,
        content,
        usage,
        context_window_tokens,
        timestamp,
    } = params;
    let mut effects = Vec::new();

    if state.cancelled_ops.contains(&op_id) {
        tracing::debug!("Ignoring model response for cancelled op {:?}", op_id);
        return effects;
    }

    let tool_calls: Vec<_> = content
        .iter()
        .filter_map(|c| {
            if let AssistantContent::ToolCall { tool_call, .. } = c {
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
        event: SessionEvent::AssistantMessageAdded {
            message,
            model: model.clone(),
        },
    });

    // Capture usage before shadowing – TokenUsage is Copy.
    let outer_usage = usage;

    if let Some(usage) = outer_usage {
        let context_window = derive_context_window_usage(usage.total_tokens, context_window_tokens);
        state.record_llm_usage(op_id, model.clone(), usage, context_window.clone());
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::LlmUsageUpdated {
                op_id,
                model: model.clone(),
                usage,
                context_window,
            },
        });
    }

    if tool_calls.is_empty() {
        state.complete_operation(op_id);
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::OperationCompleted { op_id },
        });
        // Try auto-compact first; if it doesn't fire, drain queued work.
        let auto = maybe_auto_compact(
            state,
            session_id,
            outer_usage,
            context_window_tokens,
            &model,
        );
        if auto.is_empty() {
            effects.extend(maybe_start_queued_work(state, session_id));
        } else {
            effects.extend(auto);
        }
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

    state.complete_operation(op_id);

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

    effects.extend(maybe_start_queued_work(state, session_id));

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

    if state.has_active_operation() {
        state.queue_bash_command(crate::app::domain::state::QueuedBashCommand {
            command,
            op_id,
            message_id,
            queued_at: timestamp,
        });
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::QueueUpdated {
                queue: snapshot_queue(state),
            },
        });
        return effects;
    }

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

fn handle_dequeue_queued_item(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
) -> Result<Vec<Effect>, ReduceError> {
    if state.pop_next_queued_work().is_some() {
        Ok(vec![Effect::EmitEvent {
            session_id,
            event: SessionEvent::QueueUpdated {
                queue: snapshot_queue(state),
            },
        }])
    } else {
        Err(invalid_action(
            InvalidActionKind::QueueEmpty,
            "No queued item to remove.",
        ))
    }
}

fn maybe_auto_compact(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    usage: Option<TokenUsage>,
    context_window_tokens: Option<u32>,
    model: &crate::config::model::ModelId,
) -> Vec<Effect> {
    // Guard 1: config exists and auto-compaction is enabled.
    let config = match &state.session_config {
        Some(c) if c.auto_compaction.enabled => c,
        _ => return vec![],
    };
    let threshold = config.auto_compaction.threshold_ratio();

    // Guard 2: we have usage data.
    let usage = match usage {
        Some(u) => u,
        None => return vec![],
    };

    // Guard 3: context window utilization is above threshold (non-estimated).
    let cw = match derive_context_window_usage(usage.total_tokens, context_window_tokens) {
        Some(cw) if !cw.estimated => cw,
        _ => return vec![],
    };
    let ratio = match cw.utilization_ratio {
        Some(r) if r >= threshold => r,
        _ => return vec![],
    };
    let _ = ratio; // used only in guard

    // Guard 4: enough messages to compact.
    if state.message_graph.get_thread_messages().len() < MIN_MESSAGES_FOR_COMPACT {
        return vec![];
    }

    // Guard 5: no queued work pending.
    if !state.queued_work.is_empty() {
        return vec![];
    }

    // Guard 6: no active operation (operation was completed before this call).
    if state.has_active_operation() {
        return vec![];
    }

    let op_id = crate::app::domain::types::OpId::new();
    let kind = OperationKind::Compact {
        trigger: CompactTrigger::Auto,
    };
    state.start_operation(op_id, kind.clone());
    state.operation_models.insert(op_id, model.clone());

    vec![
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::OperationStarted { op_id, kind },
        },
        Effect::RequestCompaction {
            session_id,
            op_id,
            model: model.clone(),
        },
    ]
}

fn maybe_start_queued_work(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
) -> Vec<Effect> {
    if state.has_active_operation() {
        return vec![];
    }

    let Some(next) = state.pop_next_queued_work() else {
        return vec![];
    };

    let mut effects = vec![Effect::EmitEvent {
        session_id,
        event: SessionEvent::QueueUpdated {
            queue: snapshot_queue(state),
        },
    }];

    match next {
        QueuedWorkItem::UserMessage(item) => {
            effects.extend(handle_user_input(
                state,
                session_id,
                item.content,
                item.op_id,
                item.message_id,
                item.model,
                item.queued_at,
            ));
        }
        QueuedWorkItem::DirectBash(item) => {
            effects.extend(handle_direct_bash(
                state,
                session_id,
                item.op_id,
                item.message_id,
                item.command,
                item.queued_at,
            ));
        }
    }

    effects
}

fn snapshot_queue(state: &AppState) -> Vec<QueuedWorkItemSnapshot> {
    state
        .queued_work
        .iter()
        .map(snapshot_queued_work_item)
        .collect()
}

fn snapshot_queued_work_item(item: &QueuedWorkItem) -> QueuedWorkItemSnapshot {
    match item {
        QueuedWorkItem::UserMessage(message) => QueuedWorkItemSnapshot {
            kind: Some(QueuedWorkKind::UserMessage),
            content: message
                .content
                .iter()
                .filter_map(|item| match item {
                    UserContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            queued_at: message.queued_at,
            model: Some(message.model.clone()),
            op_id: message.op_id,
            message_id: message.message_id.clone(),
            attachment_count: message
                .content
                .iter()
                .filter(|item| matches!(item, UserContent::Image { .. }))
                .count() as u32,
        },
        QueuedWorkItem::DirectBash(command) => QueuedWorkItemSnapshot {
            kind: Some(QueuedWorkKind::DirectBash),
            content: command.command.clone(),
            queued_at: command.queued_at,
            model: None,
            op_id: command.op_id,
            message_id: command.message_id.clone(),
            attachment_count: 0,
        },
    }
}

fn handle_request_compaction(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    op_id: crate::app::domain::types::OpId,
    model: crate::config::model::ModelId,
) -> Result<Vec<Effect>, ReduceError> {
    let message_count = state.message_graph.get_thread_messages().len();

    if state.has_active_operation() {
        return Err(invalid_action(
            InvalidActionKind::OperationInFlight,
            "Cannot compact while an operation is active.",
        ));
    }

    if message_count < MIN_MESSAGES_FOR_COMPACT {
        return Ok(vec![Effect::EmitEvent {
            session_id,
            event: SessionEvent::CompactResult {
                result: crate::app::domain::event::CompactResult::InsufficientMessages,
                trigger: CompactTrigger::Manual,
            },
        }]);
    }

    let kind = OperationKind::Compact {
        trigger: CompactTrigger::Manual,
    };
    state.start_operation(op_id, kind.clone());
    state.operation_models.insert(op_id, model.clone());

    Ok(vec![
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::OperationStarted { op_id, kind },
        },
        Effect::RequestCompaction {
            session_id,
            op_id,
            model,
        },
    ])
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

    if let OperationKind::Compact { trigger } = op.kind {
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::CompactResult {
                result: crate::app::domain::event::CompactResult::Cancelled,
                trigger,
            },
        });
    }

    let pending_approval = state.pending_approval.take();
    let queued_approvals = std::mem::take(&mut state.approval_queue);

    if matches!(op.kind, OperationKind::AgentLoop) {
        if let Some(pending) = pending_approval {
            let tool_name = pending.tool_call.name.clone();
            effects.extend(emit_tool_failure_message(
                state,
                session_id,
                &pending.tool_call.id,
                &tool_name,
                ToolError::Cancelled(tool_name.clone()),
                format!("Tool '{tool_name}' cancelled"),
                "cancelled",
            ));
        }

        for queued in queued_approvals {
            let tool_name = queued.tool_call.name.clone();
            effects.extend(emit_tool_failure_message(
                state,
                session_id,
                &queued.tool_call.id,
                &tool_name,
                ToolError::Cancelled(tool_name.clone()),
                format!("Tool '{tool_name}' cancelled"),
                "cancelled",
            ));
        }

        for tool_call_id in &op.pending_tool_calls {
            let tool_name = state
                .message_graph
                .find_tool_name_by_id(tool_call_id.as_str())
                .unwrap_or_else(|| tool_call_id.as_str().to_string());
            let event_error = if tool_name == tool_call_id.as_str() {
                format!("Tool call '{tool_call_id}' cancelled")
            } else {
                format!("Tool '{tool_name}' cancelled")
            };
            effects.extend(emit_tool_failure_message(
                state,
                session_id,
                tool_call_id.as_str(),
                &tool_name,
                ToolError::Cancelled(tool_name.clone()),
                event_error,
                "cancelled",
            ));
        }
    }
    state.active_streams.remove(&op.op_id);

    let dequeued_item = state.pop_next_queued_work();
    let popped_queued_item = dequeued_item.as_ref().map(snapshot_queued_work_item);

    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::OperationCancelled {
            op_id: op.op_id,
            info: CancellationInfo {
                pending_tool_calls: op.pending_tool_calls.len(),
                popped_queued_item,
            },
        },
    });

    effects.push(Effect::CancelOperation {
        session_id,
        op_id: op.op_id,
    });

    state.complete_operation(op.op_id);

    if dequeued_item.is_some() {
        effects.push(Effect::EmitEvent {
            session_id,
            event: SessionEvent::QueueUpdated {
                queue: snapshot_queue(state),
            },
        });
    }

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

fn handle_switch_primary_agent(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    agent_id: String,
) -> Result<Vec<Effect>, ReduceError> {
    if state.current_operation.is_some() {
        return Err(invalid_action(
            InvalidActionKind::OperationInFlight,
            "Cannot switch primary agent while an operation is active.",
        ));
    }

    let Some(base_config) = state
        .base_session_config
        .as_ref()
        .or(state.session_config.as_ref())
    else {
        return Err(invalid_action(
            InvalidActionKind::MissingSessionConfig,
            "Cannot switch primary agent without session config.",
        ));
    };

    let Some(_spec) = primary_agent_spec(&agent_id) else {
        return Err(invalid_action(
            InvalidActionKind::UnknownPrimaryAgent,
            format!("Unknown primary agent '{agent_id}'."),
        ));
    };

    let mut updated_config = base_config.clone();
    updated_config.primary_agent_id = Some(agent_id.clone());
    let new_config = resolve_effective_config(&updated_config);
    let backend_effects = mcp_backend_diff_effects(session_id, base_config, &new_config);

    apply_session_config_state(state, &new_config, Some(agent_id.clone()), false);

    let mut effects = Vec::new();
    effects.push(Effect::EmitEvent {
        session_id,
        event: SessionEvent::SessionConfigUpdated {
            config: Box::new(new_config),
            primary_agent_id: agent_id,
        },
    });
    effects.extend(backend_effects);
    effects.push(Effect::ReloadToolSchemas { session_id });

    Ok(effects)
}

fn apply_session_config_state(
    state: &mut AppState,
    config: &crate::session::state::SessionConfig,
    primary_agent_id: Option<String>,
    update_base: bool,
) {
    state.apply_session_config(config, primary_agent_id, update_base);
}

fn mcp_backend_diff_effects(
    session_id: crate::app::domain::types::SessionId,
    old_config: &crate::session::state::SessionConfig,
    new_config: &crate::session::state::SessionConfig,
) -> Vec<Effect> {
    let old_map = collect_mcp_backends(old_config);
    let new_map = collect_mcp_backends(new_config);

    let mut effects = Vec::new();

    for (server_name, (old_transport, old_filter)) in &old_map {
        match new_map.get(server_name) {
            None => {
                effects.push(Effect::DisconnectMcpServer {
                    session_id,
                    server_name: server_name.clone(),
                });
            }
            Some((new_transport, new_filter)) => {
                if new_transport != old_transport || new_filter != old_filter {
                    effects.push(Effect::DisconnectMcpServer {
                        session_id,
                        server_name: server_name.clone(),
                    });
                    effects.push(Effect::ConnectMcpServer {
                        session_id,
                        config: McpServerConfig {
                            server_name: server_name.clone(),
                            transport: new_transport.clone(),
                            tool_filter: new_filter.clone(),
                        },
                    });
                }
            }
        }
    }

    for (server_name, (new_transport, new_filter)) in &new_map {
        if !old_map.contains_key(server_name) {
            effects.push(Effect::ConnectMcpServer {
                session_id,
                config: McpServerConfig {
                    server_name: server_name.clone(),
                    transport: new_transport.clone(),
                    tool_filter: new_filter.clone(),
                },
            });
        }
    }

    effects
}

fn collect_mcp_backends(
    config: &crate::session::state::SessionConfig,
) -> std::collections::HashMap<
    String,
    (
        crate::tools::McpTransport,
        crate::session::state::ToolFilter,
    ),
> {
    let mut map = std::collections::HashMap::new();

    for backend_config in &config.tool_config.backends {
        let BackendConfig::Mcp {
            server_name,
            transport,
            tool_filter,
        } = backend_config;

        map.insert(
            server_name.clone(),
            (transport.clone(), tool_filter.clone()),
        );
    }

    map
}

pub fn apply_event_to_state(state: &mut AppState, event: &SessionEvent) {
    match event {
        SessionEvent::SessionCreated { config, .. } => {
            let primary_agent_id = config
                .primary_agent_id
                .clone()
                .unwrap_or_else(|| default_primary_agent_id().to_string());
            apply_session_config_state(state, config, Some(primary_agent_id), true);
        }
        SessionEvent::SessionConfigUpdated {
            config,
            primary_agent_id,
        } => {
            apply_session_config_state(state, config, Some(primary_agent_id.clone()), false);
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
            if *decision == ApprovalDecision::Approved
                && let Some(memory) = remember
            {
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
            state.pending_approval = None;
        }
        SessionEvent::OperationCompleted { op_id } => {
            state.complete_operation(*op_id);
        }
        SessionEvent::OperationCancelled { op_id, .. } => {
            state.record_cancelled_op(*op_id);
            state.complete_operation(*op_id);
        }
        SessionEvent::LlmUsageUpdated {
            op_id,
            model,
            usage,
            context_window,
        } => {
            state.record_llm_usage(*op_id, model.clone(), *usage, context_window.clone());
        }
        SessionEvent::McpServerStateChanged {
            server_name,
            state: mcp_state,
        } => {
            state
                .mcp_servers
                .insert(server_name.clone(), mcp_state.clone());
        }
        SessionEvent::QueueUpdated { queue } => {
            let parse_content = |content: &str| {
                if content.trim().is_empty() {
                    None
                } else {
                    Some(vec![UserContent::Text {
                        text: content.to_string(),
                    }])
                }
            };

            state.queued_work = queue
                .iter()
                .filter_map(|item| match item.kind {
                    Some(QueuedWorkKind::UserMessage) => {
                        let content = parse_content(item.content.as_str())?;
                        Some(QueuedWorkItem::UserMessage(
                            crate::app::domain::state::QueuedUserMessage {
                                content,
                                op_id: item.op_id,
                                message_id: item.message_id.clone(),
                                model: item.model.clone().unwrap_or_else(
                                    crate::config::model::builtin::claude_sonnet_4_5,
                                ),
                                queued_at: item.queued_at,
                            },
                        ))
                    }
                    Some(QueuedWorkKind::DirectBash) => Some(QueuedWorkItem::DirectBash(
                        crate::app::domain::state::QueuedBashCommand {
                            command: item.content.clone(),
                            op_id: item.op_id,
                            message_id: item.message_id.clone(),
                            queued_at: item.queued_at,
                        },
                    )),
                    None => {
                        let content = parse_content(item.content.as_str())?;
                        Some(QueuedWorkItem::UserMessage(
                            crate::app::domain::state::QueuedUserMessage {
                                content,
                                op_id: item.op_id,
                                message_id: item.message_id.clone(),
                                model: item.model.clone().unwrap_or_else(
                                    crate::config::model::builtin::claude_sonnet_4_5,
                                ),
                                queued_at: item.queued_at,
                            },
                        ))
                    }
                })
                .collect();
        }
        SessionEvent::ConversationCompacted { record } => {
            state
                .compaction_summary_ids
                .insert(record.summary_message_id.to_string());
            state
                .message_graph
                .mark_compaction_summary(record.summary_message_id.to_string());
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

    // Mark the summary so get_active_thread() stops here (LLM won't see older messages).
    state
        .compaction_summary_ids
        .insert(summary_message_id.to_string());
    state
        .message_graph
        .mark_compaction_summary(summary_message_id.to_string());

    let record = CompactionRecord::with_timestamp(
        compaction_id,
        summary_message_id,
        compacted_head_message_id,
        previous_active_message_id,
        model_name,
        timestamp,
    );

    let model = if let Some(model) = state.operation_models.get(&op_id).cloned() {
        model
    } else {
        state.complete_operation(op_id);
        return vec![Effect::EmitEvent {
            session_id,
            event: SessionEvent::Error {
                message: format!("Missing model for compaction operation {op_id}"),
            },
        }];
    };

    let trigger = state
        .current_operation
        .as_ref()
        .and_then(|op| match op.kind {
            OperationKind::Compact { trigger } => Some(trigger),
            _ => None,
        })
        .unwrap_or(CompactTrigger::Manual);

    state.complete_operation(op_id);

    let mut effects = vec![
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
                trigger,
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
    ];

    effects.extend(maybe_start_queued_work(state, session_id));

    effects
}

fn handle_compaction_failed(
    state: &mut AppState,
    session_id: crate::app::domain::types::SessionId,
    op_id: crate::app::domain::types::OpId,
    error: String,
) -> Vec<Effect> {
    let trigger = state
        .current_operation
        .as_ref()
        .and_then(|op| match op.kind {
            OperationKind::Compact { trigger } => Some(trigger),
            _ => None,
        })
        .unwrap_or(CompactTrigger::Manual);

    state.complete_operation(op_id);

    let mut effects = vec![
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::CompactResult {
                result: crate::app::domain::event::CompactResult::Failed(error),
                trigger,
            },
        },
        Effect::EmitEvent {
            session_id,
            event: SessionEvent::OperationCompleted { op_id },
        },
    ];

    effects.extend(maybe_start_queued_work(state, session_id));

    effects
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
    use crate::api::provider::TokenUsage;
    use crate::app::domain::event::ContextWindowUsage;
    use crate::app::domain::state::{OperationState, PendingApproval};
    use crate::app::domain::types::{MessageId, OpId, RequestId, SessionId, ToolCallId};
    use crate::config::model::builtin;
    use crate::primary_agents::resolve_effective_config;
    use crate::session::state::{
        ApprovalRules, ApprovalRulesOverrides, SessionConfig, SessionPolicyOverrides,
        ToolApprovalPolicy, ToolApprovalPolicyOverrides, ToolVisibility, UnapprovedBehavior,
    };
    use crate::tools::DISPATCH_AGENT_TOOL_NAME;
    use crate::tools::static_tools::READ_ONLY_TOOL_NAMES;
    use schemars::schema_for;
    use serde_json::json;
    use std::collections::HashSet;
    use steer_tools::{InputSchema, ToolCall, ToolError, ToolSchema};

    fn test_state() -> AppState {
        AppState::new(SessionId::new())
    }

    fn test_schema(name: &str) -> ToolSchema {
        ToolSchema {
            name: name.to_string(),
            display_name: name.to_string(),
            description: String::new(),
            input_schema: InputSchema::empty_object(),
        }
    }

    fn base_session_config() -> SessionConfig {
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.primary_agent_id = Some("normal".to_string());
        config.policy_overrides = SessionPolicyOverrides::empty();
        resolve_effective_config(&config)
    }

    fn reduce(state: &mut AppState, action: Action) -> Vec<Effect> {
        super::reduce(state, action).expect("reduce failed")
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
                content: vec![UserContent::Text {
                    text: "Hello".to_string(),
                }],
                op_id,
                message_id,
                model,
                timestamp: 1_234_567_890,
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
    fn test_switch_primary_agent_updates_visibility() {
        let mut state = test_state();
        let session_id = state.session_id;
        let config = base_session_config();
        apply_session_config_state(&mut state, &config, Some("normal".to_string()), true);

        let effects = reduce(
            &mut state,
            Action::SwitchPrimaryAgent {
                session_id,
                agent_id: "plan".to_string(),
            },
        );

        let updated = state.session_config.as_ref().expect("config");
        match &updated.tool_config.visibility {
            ToolVisibility::Whitelist(allowed) => {
                assert!(allowed.contains(DISPATCH_AGENT_TOOL_NAME));
                for name in READ_ONLY_TOOL_NAMES {
                    assert!(allowed.contains(*name));
                }
                assert_eq!(allowed.len(), READ_ONLY_TOOL_NAMES.len() + 1);
            }
            other => panic!("Unexpected tool visibility: {other:?}"),
        }
        assert_eq!(state.primary_agent_id.as_deref(), Some("plan"));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::SessionConfigUpdated { .. },
                ..
            }
        )));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ReloadToolSchemas { .. }))
        );
    }

    #[test]
    fn test_switch_primary_agent_yolo_auto_approves() {
        let mut state = test_state();
        let session_id = state.session_id;
        let config = base_session_config();
        apply_session_config_state(&mut state, &config, Some("normal".to_string()), true);

        let _ = reduce(
            &mut state,
            Action::SwitchPrimaryAgent {
                session_id,
                agent_id: "yolo".to_string(),
            },
        );

        let updated = state.session_config.as_ref().expect("config");
        assert_eq!(
            updated.tool_config.approval_policy.default_behavior,
            UnapprovedBehavior::Allow
        );
    }

    #[test]
    fn test_switch_primary_agent_preserves_policy_overrides() {
        let mut state = test_state();
        let session_id = state.session_id;

        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.primary_agent_id = Some("normal".to_string());
        config.policy_overrides = SessionPolicyOverrides {
            default_model: None,
            tool_visibility: None,
            approval_policy: ToolApprovalPolicyOverrides {
                default_behavior: Some(UnapprovedBehavior::Deny),
                preapproved: ApprovalRulesOverrides::empty(),
            },
        };
        let config = resolve_effective_config(&config);
        apply_session_config_state(&mut state, &config, Some("normal".to_string()), true);

        let _ = reduce(
            &mut state,
            Action::SwitchPrimaryAgent {
                session_id,
                agent_id: "yolo".to_string(),
            },
        );

        let updated = state.session_config.as_ref().expect("config");
        assert_eq!(
            updated.tool_config.approval_policy.default_behavior,
            UnapprovedBehavior::Deny
        );
        assert_eq!(
            updated.policy_overrides.approval_policy.default_behavior,
            Some(UnapprovedBehavior::Deny)
        );
    }

    #[test]
    fn dispatch_agent_resume_is_auto_approved() {
        let mut state = test_state();
        let session_id = state.session_id;
        let config = base_session_config();
        apply_session_config_state(&mut state, &config, Some("normal".to_string()), true);

        let tool_call = ToolCall {
            id: "tc_dispatch_resume".to_string(),
            name: DISPATCH_AGENT_TOOL_NAME.to_string(),
            parameters: json!({
                "prompt": "resume work",
                "target": {
                    "session": "resume",
                    "session_id": SessionId::new().to_string()
                }
            }),
        };

        let decision = get_tool_decision(&state, &tool_call);
        assert_eq!(decision, ToolDecision::Allow);
        assert_eq!(state.session_id, session_id);
    }

    #[test]
    fn test_switch_primary_agent_restores_base_prompt() {
        let mut state = test_state();
        let session_id = state.session_id;
        let mut config = base_session_config();
        config.system_prompt = Some("base prompt".to_string());
        apply_session_config_state(&mut state, &config, Some("normal".to_string()), true);

        let _ = reduce(
            &mut state,
            Action::SwitchPrimaryAgent {
                session_id,
                agent_id: "plan".to_string(),
            },
        );

        let _ = reduce(
            &mut state,
            Action::SwitchPrimaryAgent {
                session_id,
                agent_id: "normal".to_string(),
            },
        );

        let updated = state.session_config.as_ref().expect("config");
        assert_eq!(updated.system_prompt, Some("base prompt".to_string()));
    }

    #[test]
    fn test_switch_primary_agent_blocked_during_operation() {
        let mut state = test_state();
        let session_id = state.session_id;
        let config = base_session_config();
        apply_session_config_state(&mut state, &config, Some("normal".to_string()), true);

        state.current_operation = Some(OperationState {
            op_id: OpId::new(),
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        let result = super::reduce(
            &mut state,
            Action::SwitchPrimaryAgent {
                session_id,
                agent_id: "plan".to_string(),
            },
        );

        assert!(matches!(
            result,
            Err(ReduceError::InvalidAction {
                kind: InvalidActionKind::OperationInFlight,
                ..
            })
        ));
        assert!(state.primary_agent_id.as_deref() == Some("normal"));
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
    fn test_denied_tool_request_emits_failure_message() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());

        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.tool_config.approval_policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Deny,
            preapproved: ApprovalRules::default(),
        };
        state.session_config = Some(config);

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

        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::ToolCallFailed { .. },
                ..
            }
        )));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::ToolMessageAdded { .. },
                ..
            }
        )));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::ExecuteTool { .. }))
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::RequestUserApproval { .. }))
        );
        assert!(state.pending_approval.is_none());
        assert!(state.approval_queue.is_empty());
        assert_eq!(state.message_graph.messages.len(), 1);

        match &state.message_graph.messages[0].data {
            MessageData::Tool { result, .. } => match result {
                ToolResult::Error(error) => {
                    assert!(
                        matches!(error, ToolError::DeniedByPolicy(name) if name == "test_tool")
                    );
                }
                _ => panic!("expected denied tool error"),
            },
            _ => panic!("expected tool message"),
        }
    }

    #[test]
    fn test_user_denied_tool_request_emits_failure_message() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

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
            name: "test_tool".to_string(),
            parameters: serde_json::json!({}),
        };
        let request_id = RequestId::new();
        state.pending_approval = Some(PendingApproval {
            request_id,
            tool_call: tool_call.clone(),
        });

        let effects = reduce(
            &mut state,
            Action::ToolApprovalDecided {
                session_id,
                request_id,
                decision: ApprovalDecision::Denied,
                remember: None,
            },
        );

        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::ToolCallFailed { .. },
                ..
            }
        )));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::ToolMessageAdded { .. },
                ..
            }
        )));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::ExecuteTool { .. }))
        );
        assert!(state.pending_approval.is_none());
        assert!(state.approval_queue.is_empty());
        assert_eq!(state.message_graph.messages.len(), 1);

        match &state.message_graph.messages[0].data {
            MessageData::Tool { result, .. } => match result {
                ToolResult::Error(error) => {
                    assert!(matches!(error, ToolError::DeniedByUser(name) if name == "test_tool"));
                }
                _ => panic!("expected denied tool error"),
            },
            _ => panic!("expected tool message"),
        }
    }

    #[test]
    fn test_cancel_pops_queued_item_without_auto_start() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());

        let queued_op = OpId::new();
        let queued_message_id = MessageId::from_string("queued_msg");
        let _ = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Queued message".to_string(),
                }],
                op_id: queued_op,
                message_id: queued_message_id.clone(),
                model: builtin::claude_sonnet_4_5(),
                timestamp: 1,
            },
        );

        let effects = reduce(
            &mut state,
            Action::Cancel {
                session_id,
                op_id: None,
            },
        );

        assert!(state.current_operation.is_none());
        assert!(state.queued_work.is_empty());

        let cancellation_info = effects.iter().find_map(|effect| match effect {
            Effect::EmitEvent {
                event: SessionEvent::OperationCancelled { info, .. },
                ..
            } => Some(info),
            _ => None,
        });
        let info = cancellation_info.expect("expected OperationCancelled event");
        let popped = info
            .popped_queued_item
            .as_ref()
            .expect("expected popped queued item");
        assert_eq!(popped.content, "Queued message");
        assert_eq!(popped.op_id, queued_op);
        assert_eq!(popped.message_id, queued_message_id);

        assert!(
            !effects.iter().any(|effect| matches!(
                effect,
                Effect::EmitEvent {
                    event: SessionEvent::OperationStarted { .. },
                    ..
                }
            )),
            "queued work should not auto-start on cancel"
        );
    }

    #[test]
    fn test_cancel_injects_tool_results_for_pending_calls() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "test_tool".to_string(),
            parameters: serde_json::json!({}),
        };

        state.message_graph.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: tool_call.clone(),
                    thought_signature: None,
                }],
            },
            timestamp: 0,
            id: "msg_1".to_string(),
            parent_message_id: None,
        });

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: [ToolCallId::from_string("tc_1")].into_iter().collect(),
        });
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());

        let effects = reduce(
            &mut state,
            Action::Cancel {
                session_id,
                op_id: None,
            },
        );

        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::ToolMessageAdded { .. },
                ..
            }
        )));

        let tool_message = state
            .message_graph
            .messages
            .iter()
            .find(|message| matches!(message.data, MessageData::Tool { .. }))
            .expect("tool result should be injected on cancel");

        match &tool_message.data {
            MessageData::Tool { result, .. } => match result {
                ToolResult::Error(error) => {
                    assert!(matches!(error, ToolError::Cancelled(name) if name == "test_tool"));
                }
                _ => panic!("expected cancelled tool error"),
            },
            _ => panic!("expected tool message"),
        }
    }

    #[test]
    fn test_malformed_tool_call_auto_denies() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());

        let mut properties = serde_json::Map::new();
        properties.insert("command".to_string(), json!({ "type": "string" }));

        state.tools.push(ToolSchema {
            name: "test_tool".to_string(),
            display_name: "test_tool".to_string(),
            description: String::new(),
            input_schema: InputSchema::object(properties, vec!["command".to_string()]),
        });

        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "test_tool".to_string(),
            parameters: json!({}),
        };

        let effects = reduce(
            &mut state,
            Action::ToolApprovalRequested {
                session_id,
                request_id: RequestId::new(),
                tool_call,
            },
        );

        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::ToolCallFailed { .. },
                ..
            }
        )));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::ToolMessageAdded { .. },
                ..
            }
        )));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::ExecuteTool { .. }))
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::RequestUserApproval { .. }))
        );
        assert!(state.pending_approval.is_none());
        assert!(state.approval_queue.is_empty());
        assert_eq!(state.message_graph.messages.len(), 1);

        match &state.message_graph.messages[0].data {
            MessageData::Tool { result, .. } => match result {
                ToolResult::Error(error) => {
                    assert!(matches!(error, ToolError::InvalidParams { .. }));
                }
                _ => panic!("expected invalid params tool error"),
            },
            _ => panic!("expected tool message"),
        }
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
    fn test_dispatch_agent_missing_target_auto_denies() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());

        let input_schema: InputSchema =
            schema_for!(steer_tools::tools::dispatch_agent::DispatchAgentParams).into();
        state.tools.push(ToolSchema {
            name: DISPATCH_AGENT_TOOL_NAME.to_string(),
            display_name: "Dispatch Agent".to_string(),
            description: String::new(),
            input_schema,
        });

        let tool_call = ToolCall {
            id: "tc_dispatch".to_string(),
            name: DISPATCH_AGENT_TOOL_NAME.to_string(),
            parameters: json!({ "prompt": "hello world" }),
        };

        let effects = reduce(
            &mut state,
            Action::ToolApprovalRequested {
                session_id,
                request_id: RequestId::new(),
                tool_call,
            },
        );

        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::ToolCallFailed { .. },
                ..
            }
        )));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::ToolMessageAdded { .. },
                ..
            }
        )));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::RequestUserApproval { .. }))
        );
        assert!(state.pending_approval.is_none());
        assert!(state.approval_queue.is_empty());

        match &state.message_graph.messages[0].data {
            MessageData::Tool { result, .. } => match result {
                ToolResult::Error(error) => {
                    assert!(matches!(error, ToolError::InvalidParams { .. }));
                }
                _ => panic!("expected invalid params tool error"),
            },
            _ => panic!("expected tool message"),
        }
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
                thought_signature: None,
            },
        ];

        let effects = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id,
                content,
                usage: None,
                context_window_tokens: None,
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
                usage: None,
                context_window_tokens: None,
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
        assert!(!effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::LlmUsageUpdated { .. },
                ..
            }
        )));
    }

    #[test]
    fn test_model_response_with_usage_emits_usage_event_and_updates_state() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let message_id = MessageId::new();
        let model = builtin::claude_sonnet_4_5();
        let usage = TokenUsage::new(10, 20, 30);

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });
        state.operation_models.insert(op_id, model.clone());

        let effects = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id,
                content: vec![AssistantContent::Text {
                    text: "Done".to_string(),
                }],
                usage: Some(usage),
                context_window_tokens: None,
                timestamp: 12345,
            },
        );

        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::AssistantMessageAdded { .. },
                ..
            }
        )));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::LlmUsageUpdated {
                    op_id: usage_op_id,
                    model: usage_model,
                    usage: usage_payload,
                    context_window,
                },
                ..
            } if *usage_op_id == op_id && usage_model == &model && *usage_payload == usage && context_window.is_none()
        )));

        let snapshot = state
            .llm_usage_by_op
            .get(&op_id)
            .expect("usage snapshot should be recorded");
        assert_eq!(snapshot.model, model);
        assert_eq!(snapshot.usage, usage);
        assert!(snapshot.context_window.is_none());
        assert_eq!(state.llm_usage_totals, usage);
    }

    #[test]
    fn test_model_response_with_usage_and_context_window_emits_utilization() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let message_id = MessageId::new();
        let model = builtin::claude_sonnet_4_5();
        let usage = TokenUsage::new(10, 20, 30);
        let context_window_tokens = Some(100u32);

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });
        state.operation_models.insert(op_id, model.clone());

        let effects = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id,
                content: vec![AssistantContent::Text {
                    text: "Done".to_string(),
                }],
                usage: Some(usage),
                context_window_tokens,
                timestamp: 12345,
            },
        );

        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::LlmUsageUpdated {
                    op_id: usage_op_id,
                    model: usage_model,
                    usage: usage_payload,
                    context_window,
                },
                ..
            } if *usage_op_id == op_id
                && usage_model == &model
                && *usage_payload == usage
                && matches!(context_window, Some(ContextWindowUsage {
                    max_context_tokens: Some(100),
                    remaining_tokens: Some(70),
                    utilization_ratio: Some(ratio),
                    estimated: false,
                }) if (*ratio - 0.3).abs() < 1e-9)
        )));

        let snapshot = state
            .llm_usage_by_op
            .get(&op_id)
            .expect("usage snapshot should be recorded");
        assert_eq!(snapshot.model, model);
        assert_eq!(snapshot.usage, usage);
        assert_eq!(
            snapshot.context_window,
            Some(ContextWindowUsage {
                max_context_tokens: Some(100),
                remaining_tokens: Some(70),
                utilization_ratio: Some(0.3),
                estimated: false,
            })
        );
        assert_eq!(state.llm_usage_totals, usage);
    }

    #[test]
    fn test_model_response_with_zero_context_window_marks_estimated() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();
        let message_id = MessageId::new();
        let model = builtin::claude_sonnet_4_5();
        let usage = TokenUsage::new(10, 20, 30);

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });
        state.operation_models.insert(op_id, model);

        let effects = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id,
                content: vec![AssistantContent::Text {
                    text: "Done".to_string(),
                }],
                usage: Some(usage),
                context_window_tokens: Some(0),
                timestamp: 12345,
            },
        );

        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::LlmUsageUpdated {
                    context_window,
                    ..
                },
                ..
            } if matches!(context_window, Some(ContextWindowUsage {
                max_context_tokens: Some(0),
                remaining_tokens: Some(0),
                utilization_ratio: None,
                estimated: true,
            }))
        )));
    }

    #[test]
    fn test_apply_usage_event_updates_replay_state_and_totals() {
        let mut state = test_state();
        let op_a = OpId::new();
        let op_b = OpId::new();
        let model = builtin::claude_sonnet_4_5();

        apply_event_to_state(
            &mut state,
            &SessionEvent::LlmUsageUpdated {
                op_id: op_a,
                model: model.clone(),
                usage: TokenUsage::new(3, 5, 8),
                context_window: None,
            },
        );

        assert_eq!(state.llm_usage_totals, TokenUsage::new(3, 5, 8));

        apply_event_to_state(
            &mut state,
            &SessionEvent::LlmUsageUpdated {
                op_id: op_a,
                model: model.clone(),
                usage: TokenUsage::new(7, 11, 18),
                context_window: None,
            },
        );

        assert_eq!(state.llm_usage_totals, TokenUsage::new(7, 11, 18));

        apply_event_to_state(
            &mut state,
            &SessionEvent::LlmUsageUpdated {
                op_id: op_b,
                model,
                usage: TokenUsage::new(2, 4, 6),
                context_window: None,
            },
        );

        assert_eq!(state.llm_usage_totals, TokenUsage::new(9, 15, 24));
    }

    #[test]
    fn test_out_of_order_completion_preserves_newer_operation() {
        let mut state = test_state();
        let session_id = state.session_id;
        let model = builtin::claude_sonnet_4_5();

        let op_a = OpId::new();
        let op_b = OpId::new();

        let _ = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "first".to_string(),
                }],
                op_id: op_a,
                message_id: MessageId::new(),
                model: model.clone(),
                timestamp: 1,
            },
        );

        let _ = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "second".to_string(),
                }],
                op_id: op_b,
                message_id: MessageId::new(),
                model: model.clone(),
                timestamp: 2,
            },
        );

        let _ = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id: op_a,
                message_id: MessageId::new(),
                content: vec![AssistantContent::Text {
                    text: "done A".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                timestamp: 3,
            },
        );

        assert!(
            state
                .current_operation
                .as_ref()
                .is_some_and(|op| op.op_id == op_b)
        );
        assert!(state.operation_models.contains_key(&op_b));
        assert!(!state.operation_models.contains_key(&op_a));

        let effects = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id: op_b,
                message_id: MessageId::new(),
                content: vec![AssistantContent::Text {
                    text: "done B".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                timestamp: 4,
            },
        );

        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::OperationCompleted { op_id },
                ..
            } if *op_id == op_b
        )));
        assert!(!effects.iter().any(|e| matches!(
            e,
            Effect::EmitEvent {
                event: SessionEvent::Error { message },
                ..
            } if message.contains("Missing model for operation")
        )));
    }

    #[test]
    fn test_tool_approval_does_not_call_model_before_result() {
        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

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
        let request_id = RequestId::new();
        state.pending_approval = Some(PendingApproval {
            request_id,
            tool_call: tool_call.clone(),
        });

        let effects = reduce(
            &mut state,
            Action::ToolApprovalDecided {
                session_id,
                request_id,
                decision: ApprovalDecision::Approved,
                remember: None,
            },
        );

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExecuteTool { .. }))
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::CallModel { .. }))
        );
        assert!(state.current_operation.as_ref().is_some_and(|op| {
            op.pending_tool_calls
                .contains(&ToolCallId::from_string("tc_1"))
        }));
    }

    #[test]
    fn test_mcp_tool_visibility_and_disconnect_removal() {
        let mut state = test_state();
        let session_id = state.session_id;

        let mut allowed = HashSet::new();
        allowed.insert("mcp__alpha__allowed".to_string());

        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
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

    // ── Auto-compaction reducer tests ──────────────────────────────────

    fn setup_auto_compact_state(
        enabled: bool,
        threshold_percent: u32,
        message_count: usize,
    ) -> AppState {
        use crate::session::state::AutoCompactionConfig;

        let mut state = test_state();

        // Apply session config with auto-compaction settings.
        let mut config = base_session_config();
        config.auto_compaction = AutoCompactionConfig {
            enabled,
            threshold_percent,
        };
        apply_session_config_state(&mut state, &config, Some("normal".to_string()), true);

        // Add the requested number of messages, linked via parent_message_id so
        // that `get_thread_messages` returns them all.
        let mut prev_id: Option<String> = None;
        for i in 0..message_count {
            let id = format!("msg_{}", i);
            state.message_graph.add_message(Message {
                data: MessageData::User {
                    content: vec![UserContent::Text {
                        text: format!("message {}", i),
                    }],
                },
                timestamp: i as u64,
                id: id.clone(),
                parent_message_id: prev_id.clone(),
            });
            prev_id = Some(id);
        }

        state
    }

    #[test]
    fn test_maybe_auto_compact_triggers_when_all_guards_pass() {
        let mut state = setup_auto_compact_state(true, 90, 4);
        let session_id = state.session_id;
        let model = builtin::claude_sonnet_4_5();

        let usage = Some(TokenUsage::new(80_000, 15_000, 95_000));
        let effects = maybe_auto_compact(&mut state, session_id, usage, Some(100_000), &model);

        assert!(!effects.is_empty(), "expected non-empty effects");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RequestCompaction { .. })),
            "expected Effect::RequestCompaction"
        );
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::EmitEvent {
                    event: SessionEvent::OperationStarted { .. },
                    ..
                }
            )),
            "expected OperationStarted event"
        );
    }

    #[test]
    fn test_maybe_auto_compact_disabled_config() {
        let mut state = setup_auto_compact_state(false, 90, 4);
        let session_id = state.session_id;
        let model = builtin::claude_sonnet_4_5();

        let usage = Some(TokenUsage::new(80_000, 15_000, 95_000));
        let effects = maybe_auto_compact(&mut state, session_id, usage, Some(100_000), &model);

        assert!(
            effects.is_empty(),
            "expected empty effects when auto-compaction is disabled"
        );
    }

    #[test]
    fn test_maybe_auto_compact_below_threshold() {
        let mut state = setup_auto_compact_state(true, 90, 4);
        let session_id = state.session_id;
        let model = builtin::claude_sonnet_4_5();

        // 50% utilization, well below 90% threshold.
        let usage = Some(TokenUsage::new(40_000, 10_000, 50_000));
        let effects = maybe_auto_compact(&mut state, session_id, usage, Some(100_000), &model);

        assert!(
            effects.is_empty(),
            "expected empty effects when utilization is below threshold"
        );
    }

    #[test]
    fn test_maybe_auto_compact_insufficient_messages() {
        // Only 2 messages — below MIN_MESSAGES_FOR_COMPACT (3).
        let mut state = setup_auto_compact_state(true, 90, 2);
        let session_id = state.session_id;
        let model = builtin::claude_sonnet_4_5();

        let usage = Some(TokenUsage::new(80_000, 15_000, 95_000));
        let effects = maybe_auto_compact(&mut state, session_id, usage, Some(100_000), &model);

        assert!(
            effects.is_empty(),
            "expected empty effects with insufficient messages"
        );
    }

    #[test]
    fn test_maybe_auto_compact_queued_work_blocks() {
        use crate::app::domain::state::QueuedUserMessage;

        let mut state = setup_auto_compact_state(true, 90, 4);
        let session_id = state.session_id;
        let model = builtin::claude_sonnet_4_5();

        // Add queued work so guard 5 fires.
        state.queue_user_message(QueuedUserMessage {
            op_id: OpId::new(),
            message_id: MessageId::new(),
            content: vec![UserContent::Text {
                text: "queued msg".to_string(),
            }],
            model: builtin::claude_sonnet_4_5(),
            queued_at: 0,
        });

        let usage = Some(TokenUsage::new(80_000, 15_000, 95_000));
        let effects = maybe_auto_compact(&mut state, session_id, usage, Some(100_000), &model);

        assert!(
            effects.is_empty(),
            "expected empty effects when there is queued work"
        );
    }

    #[test]
    fn test_handle_compaction_failed_emits_compact_result() {
        use crate::app::domain::event::{CompactResult, CompactTrigger};

        let mut state = test_state();
        let session_id = state.session_id;
        let op_id = OpId::new();

        // Set up an active Compact { trigger: Auto } operation.
        state.start_operation(
            op_id,
            OperationKind::Compact {
                trigger: CompactTrigger::Auto,
            },
        );
        state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());

        let effects = reduce(
            &mut state,
            Action::CompactionFailed {
                session_id,
                op_id,
                error: "test error".into(),
            },
        );

        // Should contain CompactResult::Failed with the right trigger.
        let has_compact_result = effects.iter().any(|e| {
            matches!(
                e,
                Effect::EmitEvent {
                    event: SessionEvent::CompactResult {
                        result: CompactResult::Failed(msg),
                        trigger: CompactTrigger::Auto,
                    },
                    ..
                } if msg == "test error"
            )
        });
        assert!(
            has_compact_result,
            "expected CompactResult::Failed event with Auto trigger"
        );

        // Should NOT contain a SessionEvent::Error.
        let has_error_event = effects.iter().any(|e| {
            matches!(
                e,
                Effect::EmitEvent {
                    event: SessionEvent::Error { .. },
                    ..
                }
            )
        });
        assert!(
            !has_error_event,
            "should not emit SessionEvent::Error for compaction failure"
        );
    }

    /// Helper: extract the `messages` field from the first `Effect::CallModel` in a list of effects.
    fn extract_callmodel_messages(effects: &[Effect]) -> Option<&Vec<Message>> {
        effects.iter().find_map(|e| match e {
            Effect::CallModel { messages, .. } => Some(messages),
            _ => None,
        })
    }

    /// Helper: check whether any message in the list contains the given text substring.
    fn messages_contain_text(messages: &[Message], needle: &str) -> bool {
        messages.iter().any(|m| match &m.data {
            MessageData::User { content } => content.iter().any(|c| match c {
                UserContent::Text { text } => text.contains(needle),
                _ => false,
            }),
            MessageData::Assistant { content } => content.iter().any(|c| match c {
                AssistantContent::Text { text } => text.contains(needle),
                _ => false,
            }),
            _ => false,
        })
    }

    #[test]
    fn test_compaction_full_cycle_callmodel_filters_messages() {
        use crate::app::domain::types::CompactionId;

        let mut state = test_state();
        let session_id = state.session_id;
        let model = builtin::claude_sonnet_4_5();

        // Build 2 user/assistant turns (4 messages, exceeds MIN_MESSAGES_FOR_COMPACT=3).
        // Turn 1:
        let op_id_1 = OpId::new();
        let _ = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "hello".to_string(),
                }],
                op_id: op_id_1,
                message_id: MessageId::new(),
                model: model.clone(),
                timestamp: 1,
            },
        );
        let _ = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id: op_id_1,
                message_id: MessageId::new(),
                content: vec![AssistantContent::Text {
                    text: "hi there".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                timestamp: 2,
            },
        );

        // Turn 2:
        let op_id_1b = OpId::new();
        let _ = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "how are you".to_string(),
                }],
                op_id: op_id_1b,
                message_id: MessageId::new(),
                model: model.clone(),
                timestamp: 3,
            },
        );
        let assistant_msg_id = MessageId::new();
        let _ = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id: op_id_1b,
                message_id: assistant_msg_id.clone(),
                content: vec![AssistantContent::Text {
                    text: "doing well".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                timestamp: 4,
            },
        );
        assert!(
            state.current_operation.is_none(),
            "operation should be complete after text-only response"
        );
        assert!(state.message_graph.messages.len() >= 3);

        // RequestCompaction — starts Compact operation.
        let op_id_2 = OpId::new();
        let compact_effects = reduce(
            &mut state,
            Action::RequestCompaction {
                session_id,
                op_id: op_id_2,
                model: model.clone(),
            },
        );
        assert!(
            compact_effects
                .iter()
                .any(|e| matches!(e, Effect::RequestCompaction { .. })),
            "expected Effect::RequestCompaction"
        );

        // Step 4: CompactionComplete — summary replaces old messages as boundary.
        let compaction_id = CompactionId::new();
        let summary_message_id = MessageId::new();
        let active_before = state.message_graph.active_message_id.clone();
        let _ = reduce(
            &mut state,
            Action::CompactionComplete {
                session_id,
                op_id: op_id_2,
                compaction_id,
                summary_message_id: summary_message_id.clone(),
                summary: "Summary of conversation.".to_string(),
                compacted_head_message_id: assistant_msg_id,
                previous_active_message_id: active_before.map(MessageId::from_string),
                model: "claude-sonnet-4-5-20250929".to_string(),
                timestamp: 3,
            },
        );
        assert!(
            state.compaction_summary_ids.contains(&summary_message_id.0),
            "compaction_summary_ids should contain the summary id"
        );

        // Step 5: New UserInput("new question") — triggers CallModel.
        let op_id_3 = OpId::new();
        let effects = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "new question".to_string(),
                }],
                op_id: op_id_3,
                message_id: MessageId::new(),
                model: model.clone(),
                timestamp: 10,
            },
        );

        // Assertions on CallModel messages.
        let messages = extract_callmodel_messages(&effects)
            .expect("expected Effect::CallModel from UserInput after compaction");

        assert_eq!(
            messages.len(),
            2,
            "CallModel should contain exactly 2 messages (summary + new user msg), got: {:?}",
            messages.iter().map(|m| m.id()).collect::<Vec<_>>()
        );

        // First message is the summary (assistant).
        assert!(
            matches!(&messages[0].data, MessageData::Assistant { content } if content.iter().any(|c| matches!(c, AssistantContent::Text { text } if text == "Summary of conversation."))),
            "messages[0] should be the compaction summary"
        );

        // Second message is the new user message.
        assert!(
            matches!(&messages[1].data, MessageData::User { content } if content.iter().any(|c| matches!(c, UserContent::Text { text } if text == "new question"))),
            "messages[1] should be the new user message"
        );

        // Pre-compaction messages must NOT appear.
        assert!(
            !messages_contain_text(messages, "hello"),
            "CallModel messages must not contain pre-compaction user text 'hello'"
        );
        assert!(
            !messages_contain_text(messages, "hi there"),
            "CallModel messages must not contain pre-compaction assistant text 'hi there'"
        );
        assert!(
            !messages_contain_text(messages, "how are you"),
            "CallModel messages must not contain pre-compaction user text 'how are you'"
        );
        assert!(
            !messages_contain_text(messages, "doing well"),
            "CallModel messages must not contain pre-compaction assistant text 'doing well'"
        );
    }

    #[test]
    fn test_compaction_queued_message_callmodel_filters_messages() {
        use crate::app::domain::types::CompactionId;

        let mut state = test_state();
        let session_id = state.session_id;
        let model = builtin::claude_sonnet_4_5();

        // Build initial conversation with 2 turns (4 msgs, exceeds MIN_MESSAGES_FOR_COMPACT=3).
        // Turn 1:
        let op_id_1 = OpId::new();
        let _ = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "msg A".to_string(),
                }],
                op_id: op_id_1,
                message_id: MessageId::new(),
                model: model.clone(),
                timestamp: 1,
            },
        );
        let _ = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id: op_id_1,
                message_id: MessageId::new(),
                content: vec![AssistantContent::Text {
                    text: "response A".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                timestamp: 2,
            },
        );

        // Turn 2:
        let op_id_1b = OpId::new();
        let _ = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "msg A2".to_string(),
                }],
                op_id: op_id_1b,
                message_id: MessageId::new(),
                model: model.clone(),
                timestamp: 3,
            },
        );
        let assistant_msg_id = MessageId::new();
        let _ = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id: op_id_1b,
                message_id: assistant_msg_id.clone(),
                content: vec![AssistantContent::Text {
                    text: "response A2".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                timestamp: 4,
            },
        );
        assert!(state.current_operation.is_none());

        // Start compaction — Compact operation is now active.
        let op_id_2 = OpId::new();
        let _ = reduce(
            &mut state,
            Action::RequestCompaction {
                session_id,
                op_id: op_id_2,
                model: model.clone(),
            },
        );
        assert!(
            state.has_active_operation(),
            "Compact operation should be active"
        );

        // UserInput("msg B") while compaction is in flight — should be queued.
        let op_id_3 = OpId::new();
        let queued_effects = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "msg B".to_string(),
                }],
                op_id: op_id_3,
                message_id: MessageId::new(),
                model: model.clone(),
                timestamp: 5,
            },
        );
        assert!(
            queued_effects.iter().any(|e| matches!(
                e,
                Effect::EmitEvent {
                    event: SessionEvent::QueueUpdated { .. },
                    ..
                }
            )),
            "queued user message should emit QueueUpdated"
        );

        // CompactionComplete — completes compaction AND dequeues "msg B".
        let compaction_id = CompactionId::new();
        let summary_message_id = MessageId::new();
        let active_before = state.message_graph.active_message_id.clone();
        let completion_effects = reduce(
            &mut state,
            Action::CompactionComplete {
                session_id,
                op_id: op_id_2,
                compaction_id,
                summary_message_id: summary_message_id.clone(),
                summary: "Summary of A.".to_string(),
                compacted_head_message_id: assistant_msg_id,
                previous_active_message_id: active_before.map(MessageId::from_string),
                model: "claude-sonnet-4-5-20250929".to_string(),
                timestamp: 6,
            },
        );

        // The dequeued "msg B" should produce a CallModel effect.
        let messages = extract_callmodel_messages(&completion_effects)
            .expect("CompactionComplete should dequeue 'msg B' and produce CallModel");

        assert_eq!(
            messages.len(),
            2,
            "CallModel should contain exactly 2 messages (summary + 'msg B'), got: {:?}",
            messages.iter().map(|m| m.id()).collect::<Vec<_>>()
        );

        // Summary message.
        assert!(
            messages_contain_text(messages, "Summary of A."),
            "CallModel messages should contain the compaction summary"
        );

        // Queued user message.
        assert!(
            messages_contain_text(messages, "msg B"),
            "CallModel messages should contain the dequeued 'msg B'"
        );

        // Pre-compaction messages must NOT appear.
        assert!(
            !messages_contain_text(messages, "response A"),
            "CallModel messages must not contain pre-compaction 'response A'"
        );
        assert!(
            !messages_contain_text(messages, "response A2"),
            "CallModel messages must not contain pre-compaction 'response A2'"
        );
    }

    #[test]
    fn test_compaction_multi_round_conversation_then_compact() {
        use crate::app::domain::types::CompactionId;

        let mut state = test_state();
        let session_id = state.session_id;
        let model = builtin::claude_sonnet_4_5();

        // Build 3 user/assistant turns (6 messages) via reducer actions.
        let pre_compaction_texts = [
            ("alpha user", "alpha assistant"),
            ("beta user", "beta assistant"),
            ("gamma user", "gamma assistant"),
        ];
        let mut last_assistant_msg_id = MessageId::new();
        for (i, (user_text, assistant_text)) in pre_compaction_texts.iter().enumerate() {
            let op = OpId::new();
            let _ = reduce(
                &mut state,
                Action::UserInput {
                    session_id,
                    content: vec![UserContent::Text {
                        text: (*user_text).to_string(),
                    }],
                    op_id: op,
                    message_id: MessageId::new(),
                    model: model.clone(),
                    timestamp: (i * 2 + 1) as u64,
                },
            );
            last_assistant_msg_id = MessageId::new();
            let _ = reduce(
                &mut state,
                Action::ModelResponseComplete {
                    session_id,
                    op_id: op,
                    message_id: last_assistant_msg_id.clone(),
                    content: vec![AssistantContent::Text {
                        text: assistant_text.to_string(),
                    }],
                    usage: None,
                    context_window_tokens: None,
                    timestamp: (i * 2 + 2) as u64,
                },
            );
        }
        assert_eq!(state.message_graph.messages.len(), 6);
        assert!(state.current_operation.is_none());

        // Compact the conversation.
        let op_compact = OpId::new();
        let _ = reduce(
            &mut state,
            Action::RequestCompaction {
                session_id,
                op_id: op_compact,
                model: model.clone(),
            },
        );

        let compaction_id = CompactionId::new();
        let summary_message_id = MessageId::new();
        let active_before = state.message_graph.active_message_id.clone();
        let _ = reduce(
            &mut state,
            Action::CompactionComplete {
                session_id,
                op_id: op_compact,
                compaction_id,
                summary_message_id: summary_message_id.clone(),
                summary: "Summary of greek turns.".to_string(),
                compacted_head_message_id: last_assistant_msg_id,
                previous_active_message_id: active_before.map(MessageId::from_string),
                model: "claude-sonnet-4-5-20250929".to_string(),
                timestamp: 100,
            },
        );

        // Add 2 post-compaction turns.
        let post_compaction_texts = [
            ("delta user", "delta assistant"),
            ("epsilon user", "epsilon assistant"),
        ];
        for (i, (user_text, assistant_text)) in post_compaction_texts.iter().enumerate() {
            let op = OpId::new();
            let _ = reduce(
                &mut state,
                Action::UserInput {
                    session_id,
                    content: vec![UserContent::Text {
                        text: (*user_text).to_string(),
                    }],
                    op_id: op,
                    message_id: MessageId::new(),
                    model: model.clone(),
                    timestamp: (101 + i * 2) as u64,
                },
            );
            let _ = reduce(
                &mut state,
                Action::ModelResponseComplete {
                    session_id,
                    op_id: op,
                    message_id: MessageId::new(),
                    content: vec![AssistantContent::Text {
                        text: assistant_text.to_string(),
                    }],
                    usage: None,
                    context_window_tokens: None,
                    timestamp: (102 + i * 2) as u64,
                },
            );
        }

        // Final UserInput to trigger CallModel.
        let final_op = OpId::new();
        let effects = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "final question".to_string(),
                }],
                op_id: final_op,
                message_id: MessageId::new(),
                model: model.clone(),
                timestamp: 200,
            },
        );

        let messages =
            extract_callmodel_messages(&effects).expect("expected CallModel from final UserInput");

        // Expected: summary + 4 post-compaction messages + final user = 6.
        assert_eq!(
            messages.len(),
            6,
            "CallModel should have 6 messages (summary + 4 post-compaction + final), got: {:?}",
            messages.iter().map(|m| m.id()).collect::<Vec<_>>()
        );

        // First message is the compaction summary.
        assert!(
            messages_contain_text(&messages[..1], "Summary of greek turns."),
            "first message should be the compaction summary"
        );

        // Final message is the new user question.
        assert!(
            messages_contain_text(&messages[5..], "final question"),
            "last message should be the final user question"
        );

        // Post-compaction messages should be present.
        assert!(
            messages_contain_text(messages, "delta user"),
            "should contain post-compaction user text"
        );
        assert!(
            messages_contain_text(messages, "epsilon assistant"),
            "should contain post-compaction assistant text"
        );

        // Pre-compaction messages must NOT appear.
        for (user_text, assistant_text) in &pre_compaction_texts {
            assert!(
                !messages_contain_text(messages, user_text),
                "CallModel messages must not contain pre-compaction text '{user_text}'"
            );
            assert!(
                !messages_contain_text(messages, assistant_text),
                "CallModel messages must not contain pre-compaction text '{assistant_text}'"
            );
        }
    }
}
