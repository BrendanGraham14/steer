use crate::app::{
    AppEvent, cancellation::ActiveTool,
    conversation::{Message, Role, AssistantContent, UserContent, ToolResult as ConversationToolResult, AppCommandType},
};
use tools::ToolCall;
use crate::grpc::proto::*;
use prost_types::Timestamp;
use uuid;

/// Convert AppEvent to protobuf ServerEvent
pub fn app_event_to_server_event(app_event: AppEvent, sequence_num: u64) -> ServerEvent {
    let timestamp = Some(Timestamp::from(std::time::SystemTime::now()));

    let event = match app_event {
        AppEvent::MessageAdded { message, model } => {
            let proto_message = match &message {
                Message::User { content, timestamp, id: _ } => {
                    message_added_event::Message::User(UserMessage {
                        content: content.iter().map(|user_content| match user_content {
                            UserContent::Text { text } => crate::grpc::proto::UserContent {
                                content: Some(user_content::Content::Text(text.clone())),
                            },
                            UserContent::CommandExecution { command, stdout, stderr, exit_code } => {
                                crate::grpc::proto::UserContent {
                                    content: Some(user_content::Content::CommandExecution(CommandExecution {
                                        command: command.clone(),
                                        stdout: stdout.clone(),
                                        stderr: stderr.clone(),
                                        exit_code: *exit_code,
                                    })),
                                }
                            }
                            UserContent::AppCommand { .. } => {
                                // For now, represent app commands as empty text in gRPC
                                crate::grpc::proto::UserContent {
                                    content: Some(user_content::Content::Text(String::new())),
                                }
                            }
                        }).collect(),
                        timestamp: *timestamp,
                    })
                }
                Message::Assistant { content, timestamp, id: _ } => {
                    message_added_event::Message::Assistant(AssistantMessage {
                        content: content.iter().map(|assistant_content| match assistant_content {
                            AssistantContent::Text { text } => crate::grpc::proto::AssistantContent {
                                content: Some(assistant_content::Content::Text(text.clone())),
                            },
                            AssistantContent::ToolCall { tool_call } => {
                                crate::grpc::proto::AssistantContent {
                                    content: Some(assistant_content::Content::ToolCall(crate::grpc::proto::ToolCall {
                                        id: tool_call.id.clone(),
                                        name: tool_call.name.clone(),
                                        parameters_json: serde_json::to_string(&tool_call.parameters).unwrap_or_default(),
                                    })),
                                }
                            }
                            AssistantContent::Thought { thought } => {
                                // For now, convert thoughts to text
                                crate::grpc::proto::AssistantContent {
                                    content: Some(assistant_content::Content::Text(format!("<thinking>\n{}\n</thinking>", thought.display_text()))),
                                }
                            }
                        }).collect(),
                        timestamp: *timestamp,
                    })
                }
                Message::Tool { tool_use_id, result, timestamp, id: _ } => {
                    let proto_result = match result {
                        ConversationToolResult::Success { output } => {
                            tool_result::Result::Success(output.clone())
                        }
                        ConversationToolResult::Error { error } => {
                            tool_result::Result::Error(error.clone())
                        }
                    };
                    message_added_event::Message::Tool(ToolMessage {
                        tool_use_id: tool_use_id.clone(),
                        result: Some(crate::grpc::proto::ToolResult {
                            result: Some(proto_result),
                        }),
                        timestamp: *timestamp,
                    })
                }
            };

            Some(server_event::Event::MessageAdded(MessageAddedEvent {
                message: Some(proto_message),
                id: message.id().to_string(),
                model: model.to_string(),
            }))
        }
        AppEvent::MessageUpdated { 
            id, 
            content 
        } => Some(server_event::Event::MessageUpdated(MessageUpdatedEvent { 
            id, 
            content 
        })),
        AppEvent::MessagePart { 
            id, 
            delta 
        } => Some(server_event::Event::MessagePart(MessagePartEvent { 
            id, 
            delta 
        })),
        AppEvent::ThinkingStarted => Some(server_event::Event::ThinkingStarted(ThinkingStartedEvent {})),
        AppEvent::ThinkingCompleted => Some(server_event::Event::ThinkingCompleted(ThinkingCompletedEvent {})),
        AppEvent::ToolCallStarted { 
            name, 
            id,
            model,
        } => Some(server_event::Event::ToolCallStarted(ToolCallStartedEvent { 
            name, 
            id,
            model: model.to_string(),
        })),
        AppEvent::ToolCallCompleted { 
            name, 
            result, 
            id,
            model,
        } => Some(server_event::Event::ToolCallCompleted(ToolCallCompletedEvent { 
            name, 
            result, 
            id,
            model: model.to_string(),
        })),
        AppEvent::ToolCallFailed { 
            name, 
            error, 
            id,
            model,
        } => Some(server_event::Event::ToolCallFailed(ToolCallFailedEvent { 
            name, 
            error, 
            id,
            model: model.to_string(),
        })),
        AppEvent::RequestToolApproval { 
            name, 
            parameters, 
            id 
        } => Some(server_event::Event::RequestToolApproval(RequestToolApprovalEvent { 
            name, 
            parameters_json: serde_json::to_string(&parameters).unwrap_or_default(), 
            id 
        })),
        AppEvent::CommandResponse { 
            command: _, 
            content, 
            id 
        } => Some(server_event::Event::CommandResponse(CommandResponseEvent { 
            content, 
            id 
        })),
        AppEvent::ModelChanged { 
            model 
        } => Some(server_event::Event::ModelChanged(ModelChangedEvent { 
            model: model.to_string(),
        })),
        AppEvent::Error { 
            message 
        } => Some(server_event::Event::Error(ErrorEvent { 
            message 
        })),
        AppEvent::OperationCancelled { info } => Some(server_event::Event::OperationCancelled(OperationCancelledEvent {
            info: Some(CancellationInfo {
                api_call_in_progress: info.api_call_in_progress,
                active_tools: info.active_tools.into_iter().map(|tool| tool.name).collect(),
                pending_tool_approvals: info.pending_tool_approvals,
            }),
        }))
    };

    ServerEvent {
        sequence_num,
        timestamp,
        event,
    }
}

/// Convert protobuf ServerEvent to AppEvent
pub fn server_event_to_app_event(server_event: ServerEvent) -> Option<AppEvent> {
    match server_event.event? {
        server_event::Event::MessageAdded(e) => {
            let message = match e.message? {
                message_added_event::Message::User(user_msg) => {
                    let content = user_msg.content.into_iter().filter_map(|user_content| {
                        match user_content.content? {
                            user_content::Content::Text(text) => {
                                Some(UserContent::Text { text })
                            }
                            user_content::Content::CommandExecution(cmd) => {
                                Some(UserContent::CommandExecution {
                                    command: cmd.command,
                                    stdout: cmd.stdout,
                                    stderr: cmd.stderr,
                                    exit_code: cmd.exit_code,
                                })
                            }
                        }
                    }).collect();
                    Message::User { 
                        content, 
                        timestamp: user_msg.timestamp, 
                        id: e.id.clone(),
                    }
                }
                message_added_event::Message::Assistant(assistant_msg) => {
                    let content = assistant_msg.content.into_iter().filter_map(|assistant_content| {
                        match assistant_content.content? {
                            assistant_content::Content::Text(text) => {
                                Some(AssistantContent::Text { text })
                            }
                            assistant_content::Content::ToolCall(tool_call) => {
                                let params = serde_json::from_str(&tool_call.parameters_json)
                                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                                
                                Some(AssistantContent::ToolCall { 
                                    tool_call: ToolCall {
                                        id: tool_call.id,
                                        name: tool_call.name,
                                        parameters: params,
                                    }
                                })
                            }
                            assistant_content::Content::Thought(_) => {
                                // TODO: Handle thoughts properly when we implement them
                                None
                            }
                        }
                    }).collect();
                    Message::Assistant { 
                        content, 
                        timestamp: assistant_msg.timestamp, 
                        id: e.id.clone(),
                    }
                }
                message_added_event::Message::Tool(tool_msg) => {
                    if let Some(result) = tool_msg.result {
                        let tool_result = match result.result? {
                            tool_result::Result::Success(output) => {
                                ConversationToolResult::Success { output }
                            }
                            tool_result::Result::Error(error) => {
                                ConversationToolResult::Error { error }
                            }
                        };
                        Message::Tool {
                            tool_use_id: tool_msg.tool_use_id,
                            result: tool_result,
                            timestamp: tool_msg.timestamp,
                            id: e.id.clone(),
                        }
                    } else {
                        return None;
                    }
                }
            };

            let model = { use std::str::FromStr; crate::api::Model::from_str(&e.model).unwrap_or(crate::api::Model::Claude3_7Sonnet20250219) };

            Some(AppEvent::MessageAdded { 
                message,
                model,
            })
        }
        server_event::Event::MessageUpdated(e) => Some(AppEvent::MessageUpdated { 
            id: e.id, 
            content: e.content 
        }),
        server_event::Event::MessagePart(e) => Some(AppEvent::MessagePart { 
            id: e.id, 
            delta: e.delta 
        }),
        server_event::Event::ToolCallStarted(e) => {
            let model = { use std::str::FromStr; crate::api::Model::from_str(&e.model).unwrap_or(crate::api::Model::Claude3_7Sonnet20250219) };
            Some(AppEvent::ToolCallStarted { 
                name: e.name, 
                id: e.id,
                model,
            })
        },
        server_event::Event::ToolCallCompleted(e) => {
            let model = { use std::str::FromStr; crate::api::Model::from_str(&e.model).unwrap_or(crate::api::Model::Claude3_7Sonnet20250219) };
            Some(AppEvent::ToolCallCompleted { 
                name: e.name, 
                result: e.result, 
                id: e.id,
                model,
            })
        },
        server_event::Event::ToolCallFailed(e) => {
            let model = { use std::str::FromStr; crate::api::Model::from_str(&e.model).unwrap_or(crate::api::Model::Claude3_7Sonnet20250219) };
            Some(AppEvent::ToolCallFailed { 
                name: e.name, 
                error: e.error, 
                id: e.id,
                model,
            })
        },
        server_event::Event::ThinkingStarted(_) => Some(AppEvent::ThinkingStarted),
        server_event::Event::ThinkingCompleted(_) => Some(AppEvent::ThinkingCompleted),
        server_event::Event::RequestToolApproval(e) => {
            let parameters = serde_json::from_str(&e.parameters_json)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            Some(AppEvent::RequestToolApproval { 
                name: e.name, 
                parameters, 
                id: e.id 
            })
        },
        server_event::Event::OperationCancelled(e) => {
            if let Some(info) = e.info {
                Some(AppEvent::OperationCancelled {
                    info: crate::app::cancellation::CancellationInfo {
                        api_call_in_progress: info.api_call_in_progress,
                        active_tools: info.active_tools.into_iter().map(|name| ActiveTool { 
                            name: name.clone(), 
                            id: format!("tool_{}", uuid::Uuid::new_v4()) 
                        }).collect(),
                        pending_tool_approvals: info.pending_tool_approvals,
                    }
                })
            } else {
                None
            }
        }
        server_event::Event::CommandResponse(e) => Some(AppEvent::CommandResponse { 
            command: AppCommandType::Unknown { 
                command: "unknown".to_string() 
            },
            content: e.content, 
            id: e.id 
        }),
        server_event::Event::ModelChanged(e) => {
            let model = { use std::str::FromStr; crate::api::Model::from_str(&e.model).unwrap_or(crate::api::Model::Claude3_7Sonnet20250219) };
            Some(AppEvent::ModelChanged { model })
        },
        server_event::Event::Error(e) => Some(AppEvent::Error { 
            message: e.message 
        })
    }
}

fn role_to_proto(role: Role) -> MessageRole {
    match role {
        Role::User => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
        Role::Tool => MessageRole::Tool,
    }
}

fn proto_to_role(role: MessageRole) -> Role {
    match role {
        MessageRole::User => Role::User,
        MessageRole::Assistant => Role::Assistant,
        MessageRole::Tool => Role::Tool,
        _ => Role::User, // Default for unknown roles
    }
}