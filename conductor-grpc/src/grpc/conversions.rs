use crate::grpc::error::ConversionError;
use conductor_core::api::ToolCall;
use conductor_core::app::AppEvent;
use conductor_core::app::conversation::{
    AppCommandType, AssistantContent, CommandResponse, CompactResult,
    Message as ConversationMessage, ThoughtContent, ToolResult, UserContent,
};
use conductor_core::session::state::{
    BackendConfig, ContainerRuntime, RemoteAuth, SessionConfig, SessionToolConfig,
    ToolApprovalPolicy, ToolFilter, ToolVisibility, WorkspaceConfig,
};
use conductor_proto::agent as proto;

/// Convert internal Message to protobuf
pub fn message_to_proto(message: ConversationMessage) -> proto::Message {
    let (message_variant, created_at) = match &message {
        ConversationMessage::User {
            content,
            timestamp,
            id: _,
        } => {
            let user_msg = proto::UserMessage {
                content: content.iter().map(|user_content| match user_content {
                    UserContent::Text { text } => proto::UserContent {
                        content: Some(proto::user_content::Content::Text(text.clone())),
                    },
                    UserContent::CommandExecution { command, stdout, stderr, exit_code } => {
                        proto::UserContent {
                            content: Some(proto::user_content::Content::CommandExecution(proto::CommandExecution {
                                command: command.clone(),
                                stdout: stdout.clone(),
                                stderr: stderr.clone(),
                                exit_code: *exit_code,
                            })),
                        }
                    }
                    UserContent::AppCommand { command, response } => {

                        let command_type = match command {
                            AppCommandType::Model { target } => {
                                Some(proto::app_command_type::CommandType::Model(proto::ModelCommand {
                                    target: target.clone(),
                                }))
                            }
                            AppCommandType::Clear => Some(proto::app_command_type::CommandType::Clear(true)),
                            AppCommandType::Compact => Some(proto::app_command_type::CommandType::Compact(true)),
                            AppCommandType::Cancel => Some(proto::app_command_type::CommandType::Cancel(true)),
                            AppCommandType::Help => Some(proto::app_command_type::CommandType::Help(true)),
                            AppCommandType::Unknown { command } => {
                                Some(proto::app_command_type::CommandType::Unknown(proto::UnknownCommand {
                                    command: command.clone(),
                                }))
                            }
                        };

                        let proto_response = response.as_ref().map(|resp| {
                            let response_type = match resp {
                                CommandResponse::Text(text) => {
                                    Some(proto::command_response::Response::Text(text.clone()))
                                }
                                CommandResponse::Compact(result) => {
                                    let compact_type = match result {
                                        CompactResult::Success(summary) => {
                                            Some(proto::compact_result::ResultType::Success(summary.clone()))
                                        }
                                        CompactResult::Cancelled => {
                                            Some(proto::compact_result::ResultType::Cancelled(true))
                                        }
                                        CompactResult::InsufficientMessages => {
                                            Some(proto::compact_result::ResultType::InsufficientMessages(true))
                                        }
                                    };
                                    Some(proto::command_response::Response::Compact(proto::CompactResult {
                                        result_type: compact_type,
                                    }))
                                }
                            };
                            proto::CommandResponse { response: response_type }
                        });

                        proto::UserContent {
                            content: Some(proto::user_content::Content::AppCommand(proto::AppCommand {
                                command: Some(proto::AppCommandType { command_type }),
                                response: proto_response,
                            })),
                        }
                    }
                }).collect(),
                timestamp: *timestamp,
            };
            (proto::message::Message::User(user_msg), *timestamp)
        }
        ConversationMessage::Assistant {
            content,
            timestamp,
            id: _,
        } => {
            let assistant_msg = proto::AssistantMessage {
                content: content
                    .iter()
                    .map(|assistant_content| match assistant_content {
                        AssistantContent::Text { text } => proto::AssistantContent {
                            content: Some(proto::assistant_content::Content::Text(text.clone())),
                        },
                        AssistantContent::ToolCall { tool_call } => proto::AssistantContent {
                            content: Some(proto::assistant_content::Content::ToolCall(
                                proto::ToolCall {
                                    id: tool_call.id.clone(),
                                    name: tool_call.name.clone(),
                                    parameters_json: serde_json::to_string(&tool_call.parameters)
                                        .unwrap_or_default(),
                                },
                            )),
                        },
                        AssistantContent::Thought { thought } => {
                            let thought_type = match thought {
                                ThoughtContent::Simple { text } => {
                                    Some(proto::thought_content::ThoughtType::Simple(
                                        proto::SimpleThought { text: text.clone() },
                                    ))
                                }
                                ThoughtContent::Signed { text, signature } => {
                                    Some(proto::thought_content::ThoughtType::Signed(
                                        proto::SignedThought {
                                            text: text.clone(),
                                            signature: signature.clone(),
                                        },
                                    ))
                                }
                                ThoughtContent::Redacted { data } => {
                                    Some(proto::thought_content::ThoughtType::Redacted(
                                        proto::RedactedThought { data: data.clone() },
                                    ))
                                }
                            };

                            proto::AssistantContent {
                                content: Some(proto::assistant_content::Content::Thought(
                                    proto::ThoughtContent { thought_type },
                                )),
                            }
                        }
                    })
                    .collect(),
                timestamp: *timestamp,
            };
            (
                proto::message::Message::Assistant(assistant_msg),
                *timestamp,
            )
        }
        ConversationMessage::Tool {
            tool_use_id,
            result,
            timestamp,
            id: _,
        } => {
            let proto_result = match result {
                ToolResult::Success { output } => {
                    proto::tool_result::Result::Success(output.clone())
                }
                ToolResult::Error { error } => proto::tool_result::Result::Error(error.clone()),
            };
            let tool_msg = proto::ToolMessage {
                tool_use_id: tool_use_id.clone(),
                result: Some(proto::ToolResult {
                    result: Some(proto_result),
                }),
                timestamp: *timestamp,
            };
            (proto::message::Message::Tool(tool_msg), *timestamp)
        }
    };

    proto::Message {
        id: message.id().to_string(),
        message: Some(message_variant),
        created_at: Some(prost_types::Timestamp::from(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(created_at),
        )),
        metadata: Default::default(),
    }
}

/// Convert internal ToolApprovalPolicy to protobuf
pub fn tool_approval_policy_to_proto(policy: &ToolApprovalPolicy) -> proto::ToolApprovalPolicy {
    use proto::{
        AlwaysAskPolicy, ApprovalDecision, MixedPolicy, PreApprovedPolicy,
        tool_approval_policy::Policy,
    };

    let policy_variant = match policy {
        ToolApprovalPolicy::AlwaysAsk => Policy::AlwaysAsk(AlwaysAskPolicy {
            timeout_ms: None,
            default_decision: ApprovalDecision::Deny as i32,
        }),
        ToolApprovalPolicy::PreApproved { tools } => Policy::PreApproved(PreApprovedPolicy {
            tools: tools.iter().cloned().collect(),
        }),
        ToolApprovalPolicy::Mixed {
            pre_approved,
            ask_for_others,
        } => Policy::Mixed(MixedPolicy {
            pre_approved_tools: pre_approved.iter().cloned().collect(),
            ask_for_others: *ask_for_others,
            timeout_ms: None,
            default_decision: ApprovalDecision::Deny as i32,
        }),
    };

    proto::ToolApprovalPolicy {
        policy: Some(policy_variant),
    }
}

/// Convert internal WorkspaceConfig to protobuf
pub fn workspace_config_to_proto(config: &WorkspaceConfig) -> proto::WorkspaceConfig {
    use proto::workspace_config::Config;

    let config_variant = match config {
        WorkspaceConfig::Local => Config::Local(proto::LocalWorkspaceConfig {}),
        WorkspaceConfig::Remote {
            agent_address,
            auth,
        } => Config::Remote(proto::RemoteWorkspaceConfig {
            agent_address: agent_address.clone(),
            auth: auth.as_ref().map(remote_auth_to_proto),
        }),
        WorkspaceConfig::Container { image, runtime } => {
            Config::Container(proto::ContainerWorkspaceConfig {
                image: image.clone(),
                runtime: container_runtime_to_proto(runtime) as i32,
            })
        }
    };

    proto::WorkspaceConfig {
        config: Some(config_variant),
    }
}

/// Convert internal RemoteAuth to protobuf
pub fn remote_auth_to_proto(auth: &RemoteAuth) -> proto::RemoteAuth {
    use proto::remote_auth::Auth;

    let auth_variant = match auth {
        RemoteAuth::Bearer { token } => Auth::BearerToken(token.clone()),
        RemoteAuth::ApiKey { key } => Auth::ApiKey(key.clone()),
    };

    proto::RemoteAuth {
        auth: Some(auth_variant),
    }
}

/// Convert internal ContainerRuntime to protobuf
pub fn container_runtime_to_proto(runtime: &ContainerRuntime) -> proto::ContainerRuntime {
    match runtime {
        ContainerRuntime::Docker => proto::ContainerRuntime::Docker,
        ContainerRuntime::Podman => proto::ContainerRuntime::Podman,
    }
}

/// Convert internal ToolFilter to protobuf
pub fn tool_filter_to_proto(filter: &ToolFilter) -> proto::ToolFilter {
    use proto::tool_filter::Filter;

    let filter_variant = match filter {
        ToolFilter::All => Filter::All(true),
        ToolFilter::Include(tools) => Filter::Include(proto::IncludeFilter {
            tools: tools.clone(),
        }),
        ToolFilter::Exclude(tools) => Filter::Exclude(proto::ExcludeFilter {
            tools: tools.clone(),
        }),
    };

    proto::ToolFilter {
        filter: Some(filter_variant),
    }
}

/// Convert internal ToolVisibility to protobuf
pub fn tool_visibility_to_proto(visibility: &ToolVisibility) -> proto::ToolVisibility {
    use proto::tool_visibility::Visibility;

    let visibility_variant = match visibility {
        ToolVisibility::All => Visibility::All(true),
        ToolVisibility::ReadOnly => Visibility::ReadOnly(true),
        ToolVisibility::Whitelist(tools) => Visibility::Whitelist(proto::ToolWhitelist {
            tools: tools.iter().cloned().collect(),
        }),
        ToolVisibility::Blacklist(tools) => Visibility::Blacklist(proto::ToolBlacklist {
            tools: tools.iter().cloned().collect(),
        }),
    };

    proto::ToolVisibility {
        visibility: Some(visibility_variant),
    }
}

/// Convert internal BackendConfig to protobuf
pub fn backend_config_to_proto(config: &BackendConfig) -> proto::BackendConfig {
    use proto::backend_config::Backend;

    let backend_variant = match config {
        BackendConfig::Local { tool_filter } => Backend::Local(proto::LocalBackendConfig {
            tool_filter: Some(tool_filter_to_proto(tool_filter)),
        }),
        BackendConfig::Remote {
            name,
            endpoint,
            auth,
            tool_filter,
        } => Backend::Remote(proto::RemoteBackendConfig {
            name: name.clone(),
            endpoint: endpoint.clone(),
            auth: auth.as_ref().map(remote_auth_to_proto),
            tool_filter: Some(tool_filter_to_proto(tool_filter)),
        }),
        BackendConfig::Container {
            image,
            runtime,
            tool_filter,
        } => Backend::Container(proto::ContainerBackendConfig {
            image: image.clone(),
            runtime: container_runtime_to_proto(runtime) as i32,
            tool_filter: Some(tool_filter_to_proto(tool_filter)),
        }),
        BackendConfig::Mcp {
            server_name,
            transport,
            command,
            args,
            tool_filter,
        } => Backend::Mcp(proto::McpBackendConfig {
            server_name: server_name.clone(),
            transport: transport.clone(),
            command: command.clone(),
            args: args.clone(),
            tool_filter: Some(tool_filter_to_proto(tool_filter)),
        }),
    };

    proto::BackendConfig {
        backend: Some(backend_variant),
    }
}

/// Convert internal SessionToolConfig to protobuf
pub fn session_tool_config_to_proto(config: &SessionToolConfig) -> proto::SessionToolConfig {
    proto::SessionToolConfig {
        backends: config
            .backends
            .iter()
            .map(backend_config_to_proto)
            .collect(),
        metadata: config.metadata.clone(),
        visibility: Some(tool_visibility_to_proto(&config.visibility)),
        approval_policy: Some(tool_approval_policy_to_proto(&config.approval_policy)),
    }
}

/// Convert internal SessionConfig to protobuf
pub fn session_config_to_proto(config: &SessionConfig) -> proto::SessionConfig {
    proto::SessionConfig {
        tool_policy: Some(tool_approval_policy_to_proto(
            &config.tool_config.approval_policy,
        )),
        tool_config: Some(session_tool_config_to_proto(&config.tool_config)),
        metadata: config.metadata.clone(),
        workspace_config: Some(workspace_config_to_proto(&config.workspace)),
        system_prompt: config.system_prompt.clone(),
    }
}

/// Convert from protobuf WorkspaceConfig to internal WorkspaceConfig
pub fn proto_to_workspace_config(proto_config: proto::WorkspaceConfig) -> WorkspaceConfig {
    match proto_config.config {
        Some(proto::workspace_config::Config::Local(_)) => WorkspaceConfig::Local,
        Some(proto::workspace_config::Config::Remote(remote)) => {
            let auth = remote.auth.map(|proto_auth| match proto_auth.auth {
                Some(proto::remote_auth::Auth::BearerToken(token)) => RemoteAuth::Bearer { token },
                Some(proto::remote_auth::Auth::ApiKey(key)) => RemoteAuth::ApiKey { key },
                None => RemoteAuth::ApiKey { key: String::new() }, // Default fallback
            });
            WorkspaceConfig::Remote {
                agent_address: remote.agent_address,
                auth,
            }
        }
        Some(proto::workspace_config::Config::Container(container)) => {
            let runtime = match proto::ContainerRuntime::try_from(container.runtime) {
                Ok(proto::ContainerRuntime::Docker) => ContainerRuntime::Docker,
                Ok(proto::ContainerRuntime::Podman) => ContainerRuntime::Podman,
                _ => ContainerRuntime::Docker, // Default fallback
            };
            WorkspaceConfig::Container {
                image: container.image,
                runtime,
            }
        }
        None => WorkspaceConfig::Local,
    }
}

/// Convert from protobuf ToolFilter to internal ToolFilter
pub fn proto_to_tool_filter(proto_filter: Option<proto::ToolFilter>) -> ToolFilter {
    match proto_filter {
        Some(filter) => {
            match filter.filter {
                Some(proto::tool_filter::Filter::All(_)) => ToolFilter::All,
                Some(proto::tool_filter::Filter::Include(include_filter)) => {
                    ToolFilter::Include(include_filter.tools)
                }
                Some(proto::tool_filter::Filter::Exclude(exclude_filter)) => {
                    ToolFilter::Exclude(exclude_filter.tools)
                }
                None => ToolFilter::All, // Default to all if no filter specified
            }
        }
        None => ToolFilter::All, // Default to all if no filter provided
    }
}

/// Convert from protobuf ToolVisibility to internal ToolVisibility
pub fn proto_to_tool_visibility(proto_visibility: Option<proto::ToolVisibility>) -> ToolVisibility {
    match proto_visibility {
        Some(visibility) => {
            match visibility.visibility {
                Some(proto::tool_visibility::Visibility::All(_)) => ToolVisibility::All,
                Some(proto::tool_visibility::Visibility::ReadOnly(_)) => ToolVisibility::ReadOnly,
                Some(proto::tool_visibility::Visibility::Whitelist(whitelist)) => {
                    ToolVisibility::Whitelist(whitelist.tools.into_iter().collect())
                }
                Some(proto::tool_visibility::Visibility::Blacklist(blacklist)) => {
                    ToolVisibility::Blacklist(blacklist.tools.into_iter().collect())
                }
                None => ToolVisibility::All, // Default
            }
        }
        None => ToolVisibility::All, // Default
    }
}

/// Convert from protobuf ToolApprovalPolicy to internal ToolApprovalPolicy
pub fn proto_to_tool_approval_policy(
    proto_policy: Option<proto::ToolApprovalPolicy>,
) -> ToolApprovalPolicy {
    match proto_policy {
        Some(policy) => {
            match policy.policy {
                Some(proto::tool_approval_policy::Policy::AlwaysAsk(_)) => {
                    ToolApprovalPolicy::AlwaysAsk
                }
                Some(proto::tool_approval_policy::Policy::PreApproved(pre_approved)) => {
                    ToolApprovalPolicy::PreApproved {
                        tools: pre_approved.tools.into_iter().collect(),
                    }
                }
                Some(proto::tool_approval_policy::Policy::Mixed(mixed)) => {
                    ToolApprovalPolicy::Mixed {
                        pre_approved: mixed.pre_approved_tools.into_iter().collect(),
                        ask_for_others: mixed.ask_for_others,
                    }
                }
                None => ToolApprovalPolicy::AlwaysAsk, // Default
            }
        }
        None => ToolApprovalPolicy::AlwaysAsk, // Default
    }
}

/// Convert protobuf SessionToolConfig to internal SessionToolConfig
pub fn proto_to_tool_config(proto_config: proto::SessionToolConfig) -> SessionToolConfig {
    let backends = proto_config
        .backends
        .into_iter()
        .map(|proto_backend| {
            match proto_backend.backend {
                Some(proto::backend_config::Backend::Local(local)) => BackendConfig::Local {
                    tool_filter: proto_to_tool_filter(local.tool_filter),
                },
                Some(proto::backend_config::Backend::Remote(remote)) => {
                    let auth = remote.auth.map(|proto_auth| {
                        match proto_auth.auth {
                            Some(proto::remote_auth::Auth::BearerToken(token)) => {
                                RemoteAuth::Bearer { token }
                            }
                            Some(proto::remote_auth::Auth::ApiKey(key)) => {
                                RemoteAuth::ApiKey { key }
                            }
                            None => RemoteAuth::ApiKey { key: String::new() }, // Default fallback
                        }
                    });

                    BackendConfig::Remote {
                        name: remote.name,
                        endpoint: remote.endpoint,
                        auth,
                        tool_filter: proto_to_tool_filter(remote.tool_filter),
                    }
                }
                Some(proto::backend_config::Backend::Container(container)) => {
                    let runtime = match proto::ContainerRuntime::try_from(container.runtime) {
                        Ok(proto::ContainerRuntime::Docker) => ContainerRuntime::Docker,
                        Ok(proto::ContainerRuntime::Podman) => ContainerRuntime::Podman,
                        _ => ContainerRuntime::Docker, // Default fallback
                    };
                    BackendConfig::Container {
                        image: container.image,
                        runtime,
                        tool_filter: proto_to_tool_filter(container.tool_filter),
                    }
                }
                Some(proto::backend_config::Backend::Mcp(mcp)) => BackendConfig::Mcp {
                    server_name: mcp.server_name,
                    transport: mcp.transport,
                    command: mcp.command,
                    args: mcp.args,
                    tool_filter: proto_to_tool_filter(mcp.tool_filter),
                },
                None => BackendConfig::Local {
                    tool_filter: ToolFilter::All,
                }, // Default fallback
            }
        })
        .collect();

    SessionToolConfig {
        backends,
        approval_policy: proto_to_tool_approval_policy(proto_config.approval_policy),
        visibility: proto_to_tool_visibility(proto_config.visibility),
        metadata: proto_config.metadata,
    }
}

/// Convert proto ToolCall to core ToolCall
pub fn proto_tool_call_to_core(
    proto_tool_call: &proto::ToolCall,
) -> Result<ToolCall, ConversionError> {
    let parameters = serde_json::from_str(&proto_tool_call.parameters_json)?;
    Ok(ToolCall {
        id: proto_tool_call.id.clone(),
        name: proto_tool_call.name.clone(),
        parameters,
    })
}

/// Convert protobuf Message to internal Message
pub fn proto_to_message(proto_msg: proto::Message) -> Result<ConversationMessage, ConversionError> {
    use conductor_core::app::conversation::{
        AssistantContent, ThoughtContent, ToolResult as ConversationToolResult, UserContent,
    };
    use conductor_proto::agent::{
        assistant_content, message, thought_content, tool_result, user_content,
    };

    let message_variant = proto_msg
        .message
        .ok_or_else(|| ConversionError::MissingField {
            field: "message".to_string(),
        })?;

    match message_variant {
        message::Message::User(user_msg) => {
            let content = user_msg
                .content
                .into_iter()
                .filter_map(|user_content| {
                    user_content.content.and_then(|content| match content {
                        user_content::Content::Text(text) => Some(UserContent::Text { text }),
                        user_content::Content::AppCommand(app_cmd) => {
                            use conductor_core::app::conversation::{
                                AppCommandType as AppCmdType, CommandResponse as AppCmdResponse,
                                CompactResult,
                            };
                            use conductor_proto::agent::{app_command_type, command_response};

                            let command = app_cmd.command.as_ref().and_then(|cmd| {
                                cmd.command_type.as_ref().map(|ct| match ct {
                                    app_command_type::CommandType::Model(model) => {
                                        AppCmdType::Model {
                                            target: model.target.clone(),
                                        }
                                    }
                                    app_command_type::CommandType::Clear(_) => AppCmdType::Clear,
                                    app_command_type::CommandType::Compact(_) => AppCmdType::Compact,
                                    app_command_type::CommandType::Cancel(_) => AppCmdType::Cancel,
                                    app_command_type::CommandType::Help(_) => AppCmdType::Help,
                                    app_command_type::CommandType::Unknown(unknown) => {
                                        AppCmdType::Unknown {
                                            command: unknown.command.clone(),
                                        }
                                    }
                                })
                            });

                            let response = app_cmd.response.as_ref().and_then(|resp| {
                                resp.response.as_ref().map(|rt| match rt {
                                    command_response::Response::Text(text) => {
                                        AppCmdResponse::Text(text.clone())
                                    }
                                    command_response::Response::Compact(result) => {
                                        let compact_result = result
                                            .result_type
                                            .as_ref()
                                            .map(|rt| match rt {
                                                conductor_proto::agent::compact_result::ResultType::Success(summary) => {
                                                    CompactResult::Success(summary.clone())
                                                }
                                                conductor_proto::agent::compact_result::ResultType::Cancelled(_) => {
                                                    CompactResult::Cancelled
                                                }
                                                conductor_proto::agent::compact_result::ResultType::InsufficientMessages(_) => {
                                                    CompactResult::InsufficientMessages
                                                }
                                            })
                                            .unwrap_or(CompactResult::Cancelled);
                                        AppCmdResponse::Compact(compact_result)
                                    }
                                })
                            });

                            command.map(|cmd| UserContent::AppCommand { command: cmd, response })
                        }
                        user_content::Content::CommandExecution(cmd) => {
                            Some(UserContent::CommandExecution {
                                command: cmd.command,
                                stdout: cmd.stdout,
                                stderr: cmd.stderr,
                                exit_code: cmd.exit_code,
                            })
                        }
                    })
                })
                .collect();
            Ok(ConversationMessage::User {
                content,
                timestamp: user_msg.timestamp,
                id: proto_msg.id,
            })
        }
        message::Message::Assistant(assistant_msg) => {
            let content = assistant_msg
                .content
                .into_iter()
                .filter_map(|assistant_content| {
                    assistant_content.content.and_then(|content| match content {
                        assistant_content::Content::Text(text) => {
                            Some(AssistantContent::Text { text })
                        }
                        assistant_content::Content::ToolCall(tool_call) => {
                            match proto_tool_call_to_core(&tool_call) {
                                Ok(core_tool_call) => Some(AssistantContent::ToolCall {
                                    tool_call: core_tool_call,
                                }),
                                Err(_) => None, // Skip invalid tool calls
                            }
                        }
                        assistant_content::Content::Thought(thought) => {
                            let thought_content =
                                thought.thought_type.as_ref().map(|t| match t {
                                    thought_content::ThoughtType::Simple(simple) => {
                                        ThoughtContent::Simple {
                                            text: simple.text.clone(),
                                        }
                                    }
                                    thought_content::ThoughtType::Signed(signed) => {
                                        ThoughtContent::Signed {
                                            text: signed.text.clone(),
                                            signature: signed.signature.clone(),
                                        }
                                    }
                                    thought_content::ThoughtType::Redacted(redacted) => {
                                        ThoughtContent::Redacted {
                                            data: redacted.data.clone(),
                                        }
                                    }
                                });

                            thought_content.map(|thought| AssistantContent::Thought { thought })
                        }
                    })
                })
                .collect();
            Ok(ConversationMessage::Assistant {
                content,
                timestamp: assistant_msg.timestamp,
                id: proto_msg.id,
            })
        }
        message::Message::Tool(tool_msg) => {
            if let Some(result) = tool_msg.result {
                let tool_result = match result.result {
                    Some(tool_result::Result::Success(output)) => {
                        ConversationToolResult::Success { output }
                    }
                    Some(tool_result::Result::Error(error)) => {
                        ConversationToolResult::Error { error }
                    }
                    None => {
                        return Err(ConversionError::MissingField {
                            field: "tool_result.result".to_string(),
                        });
                    }
                };
                Ok(ConversationMessage::Tool {
                    tool_use_id: tool_msg.tool_use_id,
                    result: tool_result,
                    timestamp: tool_msg.timestamp,
                    id: proto_msg.id,
                })
            } else {
                Err(ConversionError::MissingField {
                    field: "tool_msg.result".to_string(),
                })
            }
        }
    }
}

/// Convert AppEvent to protobuf ServerEvent
pub fn app_event_to_server_event(app_event: AppEvent, sequence_num: u64) -> proto::ServerEvent {
    let timestamp = Some(prost_types::Timestamp::from(std::time::SystemTime::now()));

    let event = match app_event {
        AppEvent::MessageAdded { message, model } => {
            // Use shared conversion logic to avoid duplication
            let proto_wrapper = message_to_proto(message.clone());
            // This expect is safe because message_to_proto always returns a message with Some(message_variant)
            let msg_variant = proto_wrapper
                .message
                .expect("Missing message variant after conversion");

            let proto_message = match msg_variant {
                proto::message::Message::User(user_msg) => {
                    proto::message_added_event::Message::User(user_msg)
                }
                proto::message::Message::Assistant(assistant_msg) => {
                    proto::message_added_event::Message::Assistant(assistant_msg)
                }
                proto::message::Message::Tool(tool_msg) => {
                    proto::message_added_event::Message::Tool(tool_msg)
                }
            };

            Some(proto::server_event::Event::MessageAdded(
                proto::MessageAddedEvent {
                    message: Some(proto_message),
                    id: message.id().to_string(),
                    model: model.to_string(),
                },
            ))
        }
        AppEvent::MessageUpdated { id, content } => Some(
            proto::server_event::Event::MessageUpdated(proto::MessageUpdatedEvent { id, content }),
        ),
        AppEvent::MessagePart { id, delta } => Some(proto::server_event::Event::MessagePart(
            proto::MessagePartEvent { id, delta },
        )),
        AppEvent::ThinkingStarted => Some(proto::server_event::Event::ThinkingStarted(
            proto::ThinkingStartedEvent {},
        )),
        AppEvent::ThinkingCompleted => Some(proto::server_event::Event::ThinkingCompleted(
            proto::ThinkingCompletedEvent {},
        )),
        AppEvent::ToolCallStarted { name, id, model } => Some(
            proto::server_event::Event::ToolCallStarted(proto::ToolCallStartedEvent {
                name,
                id,
                model: model.to_string(),
            }),
        ),
        AppEvent::ToolCallCompleted {
            name,
            result,
            id,
            model,
        } => Some(proto::server_event::Event::ToolCallCompleted(
            proto::ToolCallCompletedEvent {
                name,
                result,
                id,
                model: model.to_string(),
            },
        )),
        AppEvent::ToolCallFailed {
            name,
            error,
            id,
            model,
        } => Some(proto::server_event::Event::ToolCallFailed(
            proto::ToolCallFailedEvent {
                name,
                error,
                id,
                model: model.to_string(),
            },
        )),
        AppEvent::RequestToolApproval {
            name,
            parameters,
            id,
        } => Some(proto::server_event::Event::RequestToolApproval(
            proto::RequestToolApprovalEvent {
                name,
                parameters_json: serde_json::to_string(&parameters).unwrap_or_default(),
                id,
            },
        )),
        AppEvent::CommandResponse {
            command,
            response,
            id,
        } => {
            // Convert app command to proto command
            let proto_command_type = match command {
                AppCommandType::Model { target } => Some(
                    proto::app_command_type::CommandType::Model(proto::ModelCommand {
                        target: target.clone(),
                    }),
                ),
                AppCommandType::Clear => Some(proto::app_command_type::CommandType::Clear(true)),
                AppCommandType::Compact => {
                    Some(proto::app_command_type::CommandType::Compact(true))
                }
                AppCommandType::Cancel => Some(proto::app_command_type::CommandType::Cancel(true)),
                AppCommandType::Help => Some(proto::app_command_type::CommandType::Help(true)),
                AppCommandType::Unknown { command } => Some(
                    proto::app_command_type::CommandType::Unknown(proto::UnknownCommand {
                        command: command.clone(),
                    }),
                ),
            };

            // Convert app response to proto response
            let proto_response_type = match &response {
                CommandResponse::Text(text) => {
                    Some(proto::command_response::Response::Text(text.clone()))
                }
                CommandResponse::Compact(result) => {
                    let compact_type = match result {
                        CompactResult::Success(summary) => {
                            Some(proto::compact_result::ResultType::Success(summary.clone()))
                        }
                        CompactResult::Cancelled => {
                            Some(proto::compact_result::ResultType::Cancelled(true))
                        }
                        CompactResult::InsufficientMessages => Some(
                            proto::compact_result::ResultType::InsufficientMessages(true),
                        ),
                    };
                    Some(proto::command_response::Response::Compact(
                        proto::CompactResult {
                            result_type: compact_type,
                        },
                    ))
                }
            };

            // Extract content for backward compatibility
            let content = match response {
                CommandResponse::Text(msg) => msg,
                CommandResponse::Compact(result) => match result {
                    CompactResult::Success(summary) => summary,
                    CompactResult::Cancelled => "Compact command cancelled.".to_string(),
                    CompactResult::InsufficientMessages => {
                        "Not enough messages to compact (minimum 10 required).".to_string()
                    }
                },
            };

            Some(proto::server_event::Event::CommandResponse(
                proto::CommandResponseEvent {
                    content,
                    id,
                    command: Some(proto::AppCommandType {
                        command_type: proto_command_type,
                    }),
                    response: Some(proto::CommandResponse {
                        response: proto_response_type,
                    }),
                },
            ))
        }
        AppEvent::ModelChanged { model } => Some(proto::server_event::Event::ModelChanged(
            proto::ModelChangedEvent {
                model: model.to_string(),
            },
        )),
        AppEvent::Error { message } => Some(proto::server_event::Event::Error(proto::ErrorEvent {
            message,
        })),
        AppEvent::OperationCancelled { info } => Some(
            proto::server_event::Event::OperationCancelled(proto::OperationCancelledEvent {
                info: Some(proto::CancellationInfo {
                    api_call_in_progress: info.api_call_in_progress,
                    active_tools: info
                        .active_tools
                        .into_iter()
                        .map(|tool| proto::ActiveToolInfo {
                            name: tool.name,
                            id: tool.id,
                        })
                        .collect(),
                    pending_tool_approvals: info.pending_tool_approvals,
                }),
            }),
        ),
    };

    proto::ServerEvent {
        sequence_num,
        timestamp,
        event,
    }
}

fn proto_app_command_type_to_app_command_type(
    proto_command_type: &proto::app_command_type::CommandType,
) -> AppCommandType {
    use conductor_core::app::conversation::AppCommandType;

    match proto_command_type {
        proto::app_command_type::CommandType::Model(model) => AppCommandType::Model {
            target: model.target.clone(),
        },
        proto::app_command_type::CommandType::Clear(_) => AppCommandType::Clear,
        proto::app_command_type::CommandType::Compact(_) => AppCommandType::Compact,
        proto::app_command_type::CommandType::Cancel(_) => AppCommandType::Cancel,
        proto::app_command_type::CommandType::Help(_) => AppCommandType::Help,
        proto::app_command_type::CommandType::Unknown(unknown) => AppCommandType::Unknown {
            command: unknown.command.clone(),
        },
    }
}

fn proto_compact_result_type_to_compact_result(
    proto_compact_result_type: &proto::compact_result::ResultType,
) -> CompactResult {
    match proto_compact_result_type {
        proto::compact_result::ResultType::Success(summary) => {
            CompactResult::Success(summary.clone())
        }
        proto::compact_result::ResultType::Cancelled(_) => CompactResult::Cancelled,
        proto::compact_result::ResultType::InsufficientMessages(_) => {
            CompactResult::InsufficientMessages
        }
    }
}

fn proto_command_response_response_to_command_response(
    proto_command_response_response: &proto::command_response::Response,
) -> CommandResponse {
    match proto_command_response_response {
        proto::command_response::Response::Text(text) => CommandResponse::Text(text.clone()),
        proto::command_response::Response::Compact(result) => {
            let compact_result = result
                .result_type
                .as_ref()
                .map(proto_compact_result_type_to_compact_result)
                .unwrap_or(CompactResult::Cancelled);
            CommandResponse::Compact(compact_result)
        }
    }
}

/// Convert protobuf ServerEvent to AppEvent
pub fn server_event_to_app_event(server_event: proto::ServerEvent) -> Option<AppEvent> {
    use conductor_core::app::cancellation::ActiveTool;

    match server_event.event? {
        proto::server_event::Event::MessageAdded(e) => {
            let message = match e.message? {
                proto::message_added_event::Message::User(user_msg) => {
                    let content = user_msg
                        .content
                        .into_iter()
                        .filter_map(|user_content| match user_content.content? {
                            proto::user_content::Content::Text(text) => {
                                Some(UserContent::Text { text })
                            }
                            proto::user_content::Content::CommandExecution(cmd) => {
                                Some(UserContent::CommandExecution {
                                    command: cmd.command,
                                    stdout: cmd.stdout,
                                    stderr: cmd.stderr,
                                    exit_code: cmd.exit_code,
                                })
                            }
                            proto::user_content::Content::AppCommand(app_cmd) => {
                                let command = app_cmd.command.as_ref().and_then(|cmd| {
                                    cmd.command_type
                                        .as_ref()
                                        .map(proto_app_command_type_to_app_command_type)
                                });

                                let response = app_cmd.response.as_ref().and_then(|resp| {
                                    resp.response.as_ref().map(|rt| match rt {
                                        proto::command_response::Response::Text(text) => {
                                            CommandResponse::Text(text.clone())
                                        }
                                        proto::command_response::Response::Compact(result) => {
                                            let compact_result = result
                                                .result_type
                                                .as_ref()
                                                .map(proto_compact_result_type_to_compact_result)
                                                .unwrap_or(CompactResult::Cancelled);
                                            CommandResponse::Compact(compact_result)
                                        }
                                    })
                                });

                                command.map(|cmd| UserContent::AppCommand {
                                    command: cmd,
                                    response,
                                })
                            }
                        })
                        .collect();
                    ConversationMessage::User {
                        content,
                        timestamp: user_msg.timestamp,
                        id: e.id.clone(),
                    }
                }
                proto::message_added_event::Message::Assistant(assistant_msg) => {
                    let content = assistant_msg
                        .content
                        .into_iter()
                        .filter_map(|assistant_content| {
                            match assistant_content.content? {
                                proto::assistant_content::Content::Text(text) => {
                                    Some(AssistantContent::Text { text })
                                }
                                proto::assistant_content::Content::ToolCall(tool_call) => {
                                    match proto_tool_call_to_core(&tool_call) {
                                        Ok(core_tool_call) => Some(AssistantContent::ToolCall {
                                            tool_call: core_tool_call,
                                        }),
                                        Err(_) => None, // Skip invalid tool calls
                                    }
                                }
                                proto::assistant_content::Content::Thought(_) => {
                                    // TODO: Handle thoughts properly when we implement them
                                    None
                                }
                            }
                        })
                        .collect();
                    ConversationMessage::Assistant {
                        content,
                        timestamp: assistant_msg.timestamp,
                        id: e.id.clone(),
                    }
                }
                proto::message_added_event::Message::Tool(tool_msg) => {
                    if let Some(result) = tool_msg.result {
                        let tool_result = match result.result? {
                            proto::tool_result::Result::Success(output) => {
                                ToolResult::Success { output }
                            }
                            proto::tool_result::Result::Error(error) => ToolResult::Error { error },
                        };
                        ConversationMessage::Tool {
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

            let model = {
                use std::str::FromStr;
                conductor_core::api::Model::from_str(&e.model)
                    .unwrap_or(conductor_core::api::Model::Claude3_7Sonnet20250219)
            };

            Some(AppEvent::MessageAdded { message, model })
        }
        proto::server_event::Event::MessageUpdated(e) => Some(AppEvent::MessageUpdated {
            id: e.id,
            content: e.content,
        }),
        proto::server_event::Event::MessagePart(e) => Some(AppEvent::MessagePart {
            id: e.id,
            delta: e.delta,
        }),
        proto::server_event::Event::ToolCallStarted(e) => {
            let model = {
                use std::str::FromStr;
                conductor_core::api::Model::from_str(&e.model)
                    .unwrap_or(conductor_core::api::Model::Claude3_7Sonnet20250219)
            };
            Some(AppEvent::ToolCallStarted {
                name: e.name,
                id: e.id,
                model,
            })
        }
        proto::server_event::Event::ToolCallCompleted(e) => {
            let model = {
                use std::str::FromStr;
                conductor_core::api::Model::from_str(&e.model)
                    .unwrap_or(conductor_core::api::Model::Claude3_7Sonnet20250219)
            };
            Some(AppEvent::ToolCallCompleted {
                name: e.name,
                result: e.result,
                id: e.id,
                model,
            })
        }
        proto::server_event::Event::ToolCallFailed(e) => {
            let model = {
                use std::str::FromStr;
                conductor_core::api::Model::from_str(&e.model)
                    .unwrap_or(conductor_core::api::Model::Claude3_7Sonnet20250219)
            };
            Some(AppEvent::ToolCallFailed {
                name: e.name,
                error: e.error,
                id: e.id,
                model,
            })
        }
        proto::server_event::Event::ThinkingStarted(_) => Some(AppEvent::ThinkingStarted),
        proto::server_event::Event::ThinkingCompleted(_) => Some(AppEvent::ThinkingCompleted),
        proto::server_event::Event::RequestToolApproval(e) => {
            let parameters = serde_json::from_str(&e.parameters_json)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            Some(AppEvent::RequestToolApproval {
                name: e.name,
                parameters,
                id: e.id,
            })
        }
        proto::server_event::Event::OperationCancelled(e) => {
            if let Some(info) = e.info {
                Some(AppEvent::OperationCancelled {
                    info: conductor_core::app::cancellation::CancellationInfo {
                        api_call_in_progress: info.api_call_in_progress,
                        active_tools: info
                            .active_tools
                            .into_iter()
                            .map(|tool_info| ActiveTool {
                                name: tool_info.name,
                                id: tool_info.id,
                            })
                            .collect(),
                        pending_tool_approvals: info.pending_tool_approvals,
                    },
                })
            } else {
                None
            }
        }
        proto::server_event::Event::CommandResponse(e) => {
            // Convert proto command to app command
            let command = e
                .command
                .as_ref()
                .and_then(|cmd| {
                    cmd.command_type
                        .as_ref()
                        .map(proto_app_command_type_to_app_command_type)
                })
                .unwrap_or(AppCommandType::Unknown {
                    command: e.content.clone(),
                });

            // Convert proto response to app response
            let response = e
                .response
                .as_ref()
                .and_then(|resp| {
                    resp.response
                        .as_ref()
                        .map(proto_command_response_response_to_command_response)
                })
                .unwrap_or(CommandResponse::Text(e.content.clone()));

            Some(AppEvent::CommandResponse {
                command,
                response,
                id: e.id,
            })
        }
        proto::server_event::Event::ModelChanged(e) => {
            let model = {
                use std::str::FromStr;
                conductor_core::api::Model::from_str(&e.model)
                    .unwrap_or(conductor_core::api::Model::Claude3_7Sonnet20250219)
            };
            Some(AppEvent::ModelChanged { model })
        }
        proto::server_event::Event::Error(e) => Some(AppEvent::Error { message: e.message }),
    }
}
