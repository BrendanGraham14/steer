use crate::grpc::proto;
use crate::session::state::{
    BackendConfig, ContainerRuntime, RemoteAuth, SessionConfig, SessionToolConfig,
    ToolApprovalPolicy, ToolFilter, WorkspaceConfig,
};
use crate::app::conversation::{Message, UserContent, AssistantContent, ToolResult};

/// Convert internal Message to protobuf
pub fn message_to_proto(message: Message) -> proto::Message {
    let (message_variant, created_at) = match &message {
        Message::User { content, timestamp, id } => {
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
                        use crate::app::conversation::AppCommandType as AppCmdType;
                        use crate::app::conversation::CommandResponse as AppCmdResponse;
                        
                        let command_type = match command {
                            AppCmdType::Model { target } => {
                                Some(proto::app_command_type::CommandType::Model(proto::ModelCommand {
                                    target: target.clone(),
                                }))
                            }
                            AppCmdType::Clear => Some(proto::app_command_type::CommandType::Clear(true)),
                            AppCmdType::Compact => Some(proto::app_command_type::CommandType::Compact(true)),
                            AppCmdType::Cancel => Some(proto::app_command_type::CommandType::Cancel(true)),
                            AppCmdType::Help => Some(proto::app_command_type::CommandType::Help(true)),
                            AppCmdType::Unknown { command } => {
                                Some(proto::app_command_type::CommandType::Unknown(proto::UnknownCommand {
                                    command: command.clone(),
                                }))
                            }
                        };
                        
                        let proto_response = response.as_ref().map(|resp| {
                            let response_type = match resp {
                                AppCmdResponse::Text(text) => {
                                    Some(proto::command_response::Response::Text(text.clone()))
                                }
                                AppCmdResponse::Compact(result) => {
                                    use crate::app::conversation::CompactResult as AppCompactResult;
                                    let compact_type = match result {
                                        AppCompactResult::Success(summary) => {
                                            Some(proto::compact_result::ResultType::Success(summary.clone()))
                                        }
                                        AppCompactResult::Cancelled => {
                                            Some(proto::compact_result::ResultType::Cancelled(true))
                                        }
                                        AppCompactResult::InsufficientMessages => {
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
        Message::Assistant { content, timestamp, id } => {
            let assistant_msg = proto::AssistantMessage {
                content: content.iter().map(|assistant_content| match assistant_content {
                    AssistantContent::Text { text } => proto::AssistantContent {
                        content: Some(proto::assistant_content::Content::Text(text.clone())),
                    },
                    AssistantContent::ToolCall { tool_call } => {
                        proto::AssistantContent {
                            content: Some(proto::assistant_content::Content::ToolCall(proto::ToolCall {
                                id: tool_call.id.clone(),
                                name: tool_call.name.clone(),
                                parameters_json: serde_json::to_string(&tool_call.parameters).unwrap_or_default(),
                            })),
                        }
                    }
                    AssistantContent::Thought { thought } => {
                        use crate::app::conversation::ThoughtContent as AppThoughtContent;
                        
                        let thought_type = match thought {
                            AppThoughtContent::Simple { text } => {
                                Some(proto::thought_content::ThoughtType::Simple(proto::SimpleThought {
                                    text: text.clone(),
                                }))
                            }
                            AppThoughtContent::Signed { text, signature } => {
                                Some(proto::thought_content::ThoughtType::Signed(proto::SignedThought {
                                    text: text.clone(),
                                    signature: signature.clone(),
                                }))
                            }
                            AppThoughtContent::Redacted { data } => {
                                Some(proto::thought_content::ThoughtType::Redacted(proto::RedactedThought {
                                    data: data.clone(),
                                }))
                            }
                        };
                        
                        proto::AssistantContent {
                            content: Some(proto::assistant_content::Content::Thought(proto::ThoughtContent {
                                thought_type,
                            })),
                        }
                    }
                }).collect(),
                timestamp: *timestamp,
            };
            (proto::message::Message::Assistant(assistant_msg), *timestamp)
        }
        Message::Tool { tool_use_id, result, timestamp, id } => {
            let proto_result = match result {
                ToolResult::Success { output } => {
                    proto::tool_result::Result::Success(output.clone())
                }
                ToolResult::Error { error } => {
                    proto::tool_result::Result::Error(error.clone())
                }
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
        created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(created_at))),
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
