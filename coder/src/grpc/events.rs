use crate::app::{
    AppEvent, cancellation::ActiveTool, conversation::MessageContentBlock as AppContentBlock,
    conversation::Role,
};
use crate::grpc::proto::*;
use prost_types::Timestamp;
use uuid;

/// Convert AppEvent to protobuf ServerEvent
pub fn app_event_to_server_event(app_event: AppEvent, sequence_num: u64) -> ServerEvent {
    let timestamp = Some(Timestamp::from(std::time::SystemTime::now()));

    let event = match app_event {
        AppEvent::MessageAdded {
            role,
            content_blocks,
            id,
            model,
        } => Some(server_event::Event::MessageAdded(MessageAddedEvent {
            role: role_to_proto(role) as i32,
            content_blocks: content_blocks
                .into_iter()
                .map(content_block_to_proto)
                .collect(),
            id,
            model: model.as_ref().to_string(),
        })),
        AppEvent::MessageUpdated { id, content } => {
            Some(server_event::Event::MessageUpdated(MessageUpdatedEvent {
                id,
                content,
            }))
        }
        AppEvent::MessagePart { id, delta } => {
            Some(server_event::Event::MessagePart(MessagePartEvent {
                id,
                delta,
            }))
        }
        AppEvent::ToolCallStarted { name, id, model } => {
            Some(server_event::Event::ToolCallStarted(ToolCallStartedEvent {
                name,
                id,
                model: model.as_ref().to_string(),
            }))
        }
        AppEvent::ToolCallCompleted {
            name,
            result,
            id,
            model,
        } => Some(server_event::Event::ToolCallCompleted(
            ToolCallCompletedEvent {
                name,
                result,
                id,
                model: model.as_ref().to_string(),
            },
        )),
        AppEvent::ToolCallFailed {
            name,
            error,
            id,
            model,
        } => Some(server_event::Event::ToolCallFailed(ToolCallFailedEvent {
            name,
            error,
            id,
            model: model.as_ref().to_string(),
        })),
        AppEvent::ThinkingStarted => Some(server_event::Event::ThinkingStarted(
            ThinkingStartedEvent {},
        )),
        AppEvent::ThinkingCompleted => Some(server_event::Event::ThinkingCompleted(
            ThinkingCompletedEvent {},
        )),
        AppEvent::CommandResponse { content, id } => {
            Some(server_event::Event::CommandResponse(CommandResponseEvent {
                content,
                id,
            }))
        }
        AppEvent::RequestToolApproval {
            name,
            parameters,
            id,
        } => Some(server_event::Event::RequestToolApproval(
            RequestToolApprovalEvent {
                name,
                parameters_json: parameters.to_string(),
                id,
            },
        )),
        AppEvent::OperationCancelled { info } => Some(server_event::Event::OperationCancelled(
            OperationCancelledEvent {
                info: Some(CancellationInfo {
                    api_call_in_progress: info.api_call_in_progress,
                    active_tools: info
                        .active_tools
                        .into_iter()
                        .map(|tool| tool.name)
                        .collect(),
                    pending_tool_approvals: info.pending_tool_approvals,
                }),
            },
        )),
        AppEvent::ModelChanged { model } => {
            Some(server_event::Event::ModelChanged(ModelChangedEvent {
                model: model.as_ref().to_string(),
            }))
        }
        AppEvent::Error { message } => Some(server_event::Event::Error(ErrorEvent { message })),
        AppEvent::RestoredMessage {
            role,
            content_blocks,
            id,
            model,
        } => {
            // Convert RestoredMessage to RestoredMessageEvent for gRPC transmission
            // This allows remote TUIs to display conversation history without duplication
            Some(server_event::Event::RestoredMessage(RestoredMessageEvent {
                role: role_to_proto(role) as i32,
                content_blocks: content_blocks
                    .into_iter()
                    .map(content_block_to_proto)
                    .collect(),
                id,
                model: model.as_ref().to_string(),
            }))
        }
    };

    ServerEvent {
        sequence_num,
        timestamp,
        event,
    }
}

fn role_to_proto(role: Role) -> MessageRole {
    match role {
        Role::User => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
        Role::Tool => MessageRole::Tool,
    }
}

fn content_block_to_proto(
    block: crate::app::conversation::MessageContentBlock,
) -> MessageContentBlock {
    use crate::app::conversation::MessageContentBlock as AppBlock;

    match block {
        AppBlock::Text(text) => MessageContentBlock {
            content: Some(message_content_block::Content::Text(text)),
        },
        AppBlock::ToolCall(tool_call) => MessageContentBlock {
            content: Some(message_content_block::Content::ToolCall(ToolCall {
                id: tool_call.id,
                name: tool_call.name,
                parameters: match tool_call.parameters {
                    serde_json::Value::Object(map) => {
                        map.into_iter().map(|(k, v)| (k, v.to_string())).collect()
                    }
                    _ => std::collections::HashMap::new(),
                },
            })),
        },
        AppBlock::ToolResult {
            tool_use_id,
            result,
        } => MessageContentBlock {
            content: Some(message_content_block::Content::ToolResult(ToolResult {
                tool_call_id: tool_use_id,
                success: true, // In this context, we assume success (errors would be in ToolCallFailed events)
                content: result,
                error: None,
                metadata: std::collections::HashMap::new(),
            })),
        },
    }
}

/// Convert protobuf ServerEvent to AppEvent for TUI consumption
pub fn server_event_to_app_event(server_event: ServerEvent) -> Option<AppEvent> {
    

    match server_event.event? {
        server_event::Event::MessageAdded(e) => Some(AppEvent::MessageAdded {
            role: proto_to_role(MessageRole::try_from(e.role).ok()?),
            content_blocks: e
                .content_blocks
                .into_iter()
                .map(proto_to_content_block)
                .collect(),
            id: e.id,
            model: e.model.parse().ok()?,
        }),
        server_event::Event::MessageUpdated(e) => Some(AppEvent::MessageUpdated {
            id: e.id,
            content: e.content,
        }),
        server_event::Event::MessagePart(e) => Some(AppEvent::MessagePart {
            id: e.id,
            delta: e.delta,
        }),
        server_event::Event::ToolCallStarted(e) => Some(AppEvent::ToolCallStarted {
            name: e.name,
            id: e.id,
            model: e.model.parse().ok()?,
        }),
        server_event::Event::ToolCallCompleted(e) => Some(AppEvent::ToolCallCompleted {
            name: e.name,
            result: e.result,
            id: e.id,
            model: e.model.parse().ok()?,
        }),
        server_event::Event::ToolCallFailed(e) => Some(AppEvent::ToolCallFailed {
            name: e.name,
            error: e.error,
            id: e.id,
            model: e.model.parse().ok()?,
        }),
        server_event::Event::ThinkingStarted(_) => Some(AppEvent::ThinkingStarted),
        server_event::Event::ThinkingCompleted(_) => Some(AppEvent::ThinkingCompleted),
        server_event::Event::CommandResponse(e) => Some(AppEvent::CommandResponse {
            content: e.content,
            id: e.id,
        }),
        server_event::Event::RequestToolApproval(e) => {
            let parameters =
                serde_json::from_str(&e.parameters_json).unwrap_or(serde_json::Value::Null);
            Some(AppEvent::RequestToolApproval {
                name: e.name,
                parameters,
                id: e.id,
            })
        }
        server_event::Event::OperationCancelled(e) => {
            let info = if let Some(cancellation_info) = e.info {
                crate::app::cancellation::CancellationInfo {
                    api_call_in_progress: cancellation_info.api_call_in_progress,
                    active_tools: cancellation_info
                        .active_tools
                        .into_iter()
                        .map(|name| ActiveTool {
                            id: uuid::Uuid::new_v4().to_string(),
                            name,
                        })
                        .collect(),
                    pending_tool_approvals: cancellation_info.pending_tool_approvals,
                }
            } else {
                crate::app::cancellation::CancellationInfo {
                    api_call_in_progress: false,
                    active_tools: vec![],
                    pending_tool_approvals: false,
                }
            };

            Some(AppEvent::OperationCancelled { info })
        }
        server_event::Event::ModelChanged(e) => Some(AppEvent::ModelChanged {
            model: e.model.parse().ok()?,
        }),
        server_event::Event::Error(e) => Some(AppEvent::Error { message: e.message }),
        server_event::Event::RestoredMessage(e) => Some(AppEvent::RestoredMessage {
            role: proto_to_role(MessageRole::try_from(e.role).ok()?),
            content_blocks: e
                .content_blocks
                .into_iter()
                .map(proto_to_content_block)
                .collect(),
            id: e.id,
            model: e.model.parse().ok()?,
        }),
    }
}

fn proto_to_role(role: MessageRole) -> Role {
    match role {
        MessageRole::User => Role::User,
        MessageRole::Assistant => Role::Assistant,
        MessageRole::Tool => Role::Tool,
        MessageRole::System => Role::User, // Map system to user for now
        MessageRole::Unspecified => Role::User, // Default fallback
    }
}

fn proto_to_content_block(block: MessageContentBlock) -> AppContentBlock {
    match block.content {
        Some(message_content_block::Content::Text(text)) => AppContentBlock::Text(text),
        Some(message_content_block::Content::ToolCall(tool_call)) => {
            AppContentBlock::ToolCall(tools::ToolCall {
                id: tool_call.id,
                name: tool_call.name,
                parameters: {
                    // Convert string parameters back to JSON
                    let mut params = serde_json::Map::new();
                    for (k, v) in tool_call.parameters {
                        // Try to parse as JSON, fall back to string
                        let value =
                            serde_json::from_str(&v).unwrap_or(serde_json::Value::String(v));
                        params.insert(k, value);
                    }
                    serde_json::Value::Object(params)
                },
            })
        }
        Some(message_content_block::Content::ToolResult(tool_result)) => {
            AppContentBlock::ToolResult {
                tool_use_id: tool_result.tool_call_id,
                result: tool_result.content,
            }
        }
        Some(message_content_block::Content::Attachment(_)) => {
            // TODO: Handle attachments - for now, convert to text
            AppContentBlock::Text("[Attachment]".to_string())
        }
        None => AppContentBlock::Text(String::new()),
    }
}
