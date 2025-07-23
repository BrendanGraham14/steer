use crate::grpc::error::ConversionError;
use std::time::Duration;
use steer_core::api::ToolCall;
use steer_core::app::command::ApprovalType;
use steer_core::app::conversation::{
    AppCommandType, AssistantContent, CommandResponse, CompactResult,
    Message as ConversationMessage, MessageData, ThoughtContent, UserContent,
};
use steer_core::app::{AppCommand, AppEvent, BashError, CompactError, Operation, OperationOutcome};
use steer_core::session::state::{
    BackendConfig, BashToolConfig, ContainerRuntime, RemoteAuth, SessionConfig, SessionToolConfig,
    ToolApprovalPolicy, ToolFilter, ToolSpecificConfig, ToolVisibility, WorkspaceConfig,
};
use steer_proto::agent::v1 as proto;
use steer_proto::common::v1 as common;
use steer_tools::tools::todo::{TodoItem, TodoPriority, TodoStatus, TodoWriteFileOperation};

/// Convert steer_tools ToolResult to protobuf
fn steer_tools_result_to_proto(
    result: &steer_tools::result::ToolResult,
) -> Result<proto::ToolResult, ConversionError> {
    use proto::tool_result::Result as ProtoResult;
    use steer_tools::result::ToolResult as CoreResult;

    let proto_result = match result {
        CoreResult::Search(r) => ProtoResult::Search(common::SearchResult {
            matches: r
                .matches
                .iter()
                .map(|m| common::SearchMatch {
                    file_path: m.file_path.clone(),
                    line_number: m.line_number as u64,
                    line_content: m.line_content.clone(),
                    column_range: m.column_range.map(|(start, end)| common::ColumnRange {
                        start: start as u64,
                        end: end as u64,
                    }),
                })
                .collect(),
            total_files_searched: r.total_files_searched as u64,
            search_completed: r.search_completed,
        }),
        CoreResult::FileList(r) => ProtoResult::FileList(common::FileListResult {
            entries: r
                .entries
                .iter()
                .map(|e| common::FileEntry {
                    path: e.path.clone(),
                    is_directory: e.is_directory,
                    size: e.size,
                    permissions: e.permissions.clone(),
                })
                .collect(),
            base_path: r.base_path.clone(),
        }),
        CoreResult::FileContent(r) => ProtoResult::FileContent(common::FileContentResult {
            content: r.content.clone(),
            file_path: r.file_path.clone(),
            line_count: r.line_count as u64,
            truncated: r.truncated,
        }),
        CoreResult::Edit(r) => ProtoResult::Edit(common::EditResult {
            file_path: r.file_path.clone(),
            changes_made: r.changes_made as u64,
            file_created: r.file_created,
            old_content: r.old_content.clone(),
            new_content: r.new_content.clone(),
        }),
        CoreResult::Bash(r) => ProtoResult::Bash(common::BashResult {
            stdout: r.stdout.clone(),
            stderr: r.stderr.clone(),
            exit_code: r.exit_code,
            command: r.command.clone(),
        }),
        CoreResult::Glob(r) => ProtoResult::Glob(common::GlobResult {
            matches: r.matches.clone(),
            pattern: r.pattern.clone(),
        }),
        CoreResult::TodoRead(r) => ProtoResult::TodoRead(common::TodoListResult {
            todos: r.todos.iter().map(convert_todo_item_to_proto).collect(),
        }),
        CoreResult::TodoWrite(r) => ProtoResult::TodoWrite(common::TodoWriteResult {
            todos: r.todos.iter().map(convert_todo_item_to_proto).collect(),
            operation: convert_todo_write_file_operation_to_proto(&r.operation) as i32,
        }),
        CoreResult::Fetch(r) => ProtoResult::Fetch(proto::FetchResult {
            url: r.url.clone(),
            content: r.content.clone(),
        }),
        CoreResult::Agent(r) => ProtoResult::Agent(proto::AgentResult {
            content: r.content.clone(),
        }),
        CoreResult::External(r) => ProtoResult::External(proto::ExternalResult {
            tool_name: r.tool_name.clone(),
            payload: r.payload.clone(),
        }),
        CoreResult::Error(e) => ProtoResult::Error(tool_error_to_proto(e)),
    };

    Ok(proto::ToolResult {
        result: Some(proto_result),
    })
}

/// Convert ToolError to protobuf
fn tool_error_to_proto(error: &steer_tools::error::ToolError) -> proto::ToolError {
    use proto::tool_error::ErrorType;
    use steer_tools::error::ToolError;

    let error_type = match error {
        ToolError::UnknownTool(name) => ErrorType::UnknownTool(name.clone()),
        ToolError::InvalidParams(tool_name, message) => {
            ErrorType::InvalidParams(proto::InvalidParamsError {
                tool_name: tool_name.clone(),
                message: message.clone(),
            })
        }
        ToolError::Execution { tool_name, message } => {
            ErrorType::Execution(proto::ExecutionError {
                tool_name: tool_name.clone(),
                message: message.clone(),
            })
        }
        ToolError::Cancelled(name) => ErrorType::Cancelled(name.clone()),
        ToolError::Timeout(name) => ErrorType::Timeout(name.clone()),
        ToolError::DeniedByUser(name) => ErrorType::DeniedByUser(name.clone()),
        ToolError::InternalError(msg) => ErrorType::InternalError(msg.clone()),
        ToolError::Io { tool_name, message } => ErrorType::Io(proto::IoError {
            tool_name: tool_name.clone(),
            message: message.clone(),
        }),
        ToolError::Serialization(msg) => ErrorType::Serialization(msg.clone()),
        ToolError::Http(msg) => ErrorType::Http(msg.clone()),
        ToolError::Regex(msg) => ErrorType::Regex(msg.clone()),
        ToolError::McpConnectionFailed {
            server_name,
            message,
        } => ErrorType::McpConnectionFailed(proto::McpConnectionFailedError {
            server_name: server_name.clone(),
            message: message.clone(),
        }),
    };

    proto::ToolError {
        error_type: Some(error_type),
    }
}

/// Convert protobuf ToolResult to steer_tools ToolResult
fn proto_to_steer_tools_result(
    proto_result: proto::ToolResult,
) -> Result<steer_tools::result::ToolResult, ConversionError> {
    use proto::tool_result::Result as ProtoResult;
    use steer_tools::result::*;

    let result = proto_result
        .result
        .ok_or_else(|| ConversionError::MissingField {
            field: "tool_result.result".to_string(),
        })?;

    Ok(match result {
        ProtoResult::Search(r) => ToolResult::Search(SearchResult {
            matches: r
                .matches
                .into_iter()
                .map(|m| SearchMatch {
                    file_path: m.file_path,
                    line_number: m.line_number as usize,
                    line_content: m.line_content,
                    column_range: m
                        .column_range
                        .map(|cr| (cr.start as usize, cr.end as usize)),
                })
                .collect(),
            total_files_searched: r.total_files_searched as usize,
            search_completed: r.search_completed,
        }),
        ProtoResult::FileList(r) => ToolResult::FileList(FileListResult {
            entries: r
                .entries
                .into_iter()
                .map(|e| FileEntry {
                    path: e.path,
                    is_directory: e.is_directory,
                    size: e.size,
                    permissions: e.permissions,
                })
                .collect(),
            base_path: r.base_path,
        }),
        ProtoResult::FileContent(r) => ToolResult::FileContent(FileContentResult {
            content: r.content,
            file_path: r.file_path,
            line_count: r.line_count as usize,
            truncated: r.truncated,
        }),
        ProtoResult::Edit(r) => ToolResult::Edit(EditResult {
            file_path: r.file_path,
            changes_made: r.changes_made as usize,
            file_created: r.file_created,
            old_content: r.old_content,
            new_content: r.new_content,
        }),
        ProtoResult::Bash(r) => ToolResult::Bash(BashResult {
            stdout: r.stdout,
            stderr: r.stderr,
            exit_code: r.exit_code,
            command: r.command,
        }),
        ProtoResult::Glob(r) => ToolResult::Glob(GlobResult {
            matches: r.matches,
            pattern: r.pattern,
        }),
        ProtoResult::TodoRead(r) => ToolResult::TodoRead(TodoListResult {
            todos: r
                .todos
                .into_iter()
                .map(convert_proto_to_todo_item)
                .collect(),
        }),
        ProtoResult::TodoWrite(r) => ToolResult::TodoWrite(TodoWriteResult {
            todos: r
                .todos
                .into_iter()
                .map(convert_proto_to_todo_item)
                .collect(),
            operation: convert_proto_to_todo_write_file_operation(
                steer_proto::common::v1::TodoWriteFileOperation::try_from(r.operation)
                    .unwrap_or(steer_proto::common::v1::TodoWriteFileOperation::OperationUnset),
            ),
        }),
        ProtoResult::Fetch(r) => ToolResult::Fetch(FetchResult {
            url: r.url,
            content: r.content,
        }),
        ProtoResult::Agent(r) => ToolResult::Agent(AgentResult { content: r.content }),
        ProtoResult::External(r) => ToolResult::External(ExternalResult {
            tool_name: r.tool_name,
            payload: r.payload,
        }),
        ProtoResult::Error(e) => ToolResult::Error(proto_to_tool_error(e)?),
    })
}

/// Convert protobuf ToolError to steer_tools ToolError
fn proto_to_tool_error(
    proto_error: proto::ToolError,
) -> Result<steer_tools::error::ToolError, ConversionError> {
    use proto::tool_error::ErrorType;
    use steer_tools::error::ToolError;

    let error_type = proto_error
        .error_type
        .ok_or_else(|| ConversionError::MissingField {
            field: "tool_error.error_type".to_string(),
        })?;

    Ok(match error_type {
        ErrorType::UnknownTool(name) => ToolError::UnknownTool(name),
        ErrorType::InvalidParams(e) => ToolError::InvalidParams(e.tool_name, e.message),
        ErrorType::Execution(e) => ToolError::Execution {
            tool_name: e.tool_name,
            message: e.message,
        },
        ErrorType::Cancelled(name) => ToolError::Cancelled(name),
        ErrorType::Timeout(name) => ToolError::Timeout(name),
        ErrorType::DeniedByUser(name) => ToolError::DeniedByUser(name),
        ErrorType::InternalError(msg) => ToolError::InternalError(msg),
        ErrorType::Io(e) => ToolError::Io {
            tool_name: e.tool_name,
            message: e.message,
        },
        ErrorType::Serialization(msg) => ToolError::Serialization(msg),
        ErrorType::Http(msg) => ToolError::Http(msg),
        ErrorType::Regex(msg) => ToolError::Regex(msg),
        ErrorType::McpConnectionFailed(e) => ToolError::McpConnectionFailed {
            server_name: e.server_name,
            message: e.message,
        },
    })
}

/// Convert internal Message to protobuf
pub fn message_to_proto(message: ConversationMessage) -> Result<proto::Message, ConversionError> {
    let (message_variant, created_at) = match &message.data {
        MessageData::User { content, .. } => {
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
                timestamp: message.timestamp,
                parent_message_id: message.parent_message_id.clone(),
            };
            (proto::message::Message::User(user_msg), message.timestamp)
        }
        MessageData::Assistant { content, .. } => {
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
                timestamp: message.timestamp,
                parent_message_id: message.parent_message_id.clone(),
            };
            (
                proto::message::Message::Assistant(assistant_msg),
                message.timestamp,
            )
        }
        MessageData::Tool {
            tool_use_id,
            result,
            ..
        } => {
            let proto_result = steer_tools_result_to_proto(result)?;
            let tool_msg = proto::ToolMessage {
                tool_use_id: tool_use_id.clone(),
                result: Some(proto_result),
                timestamp: message.timestamp,
                parent_message_id: message.parent_message_id.clone(),
            };
            (proto::message::Message::Tool(tool_msg), message.timestamp)
        }
    };

    Ok(proto::Message {
        id: message.id().to_string(),
        message: Some(message_variant),
        created_at: Some(prost_types::Timestamp::from(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(created_at),
        )),
        metadata: Default::default(),
    })
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
            default_decision: Some(ApprovalDecision {
                decision_type: Some(proto::approval_decision::DecisionType::Deny(true)),
            }),
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
            default_decision: Some(ApprovalDecision {
                decision_type: Some(proto::approval_decision::DecisionType::Deny(true)),
            }),
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
        WorkspaceConfig::Local { path } => Config::Local(proto::LocalWorkspaceConfig {
            path: path.display().to_string(),
        }),
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
        BackendConfig::Mcp {
            server_name,
            transport,
            tool_filter,
        } => Backend::Mcp(proto::McpBackendConfig {
            server_name: server_name.clone(),
            // Serialize the McpTransport enum to JSON string for proto compatibility
            transport: serde_json::to_string(&transport).unwrap_or_else(|_| "{}".to_string()),
            // Proto still expects command and args separately, extract from transport
            command: match transport {
                steer_core::tools::McpTransport::Stdio { command, .. } => command.clone(),
                _ => String::new(),
            },
            args: match transport {
                steer_core::tools::McpTransport::Stdio { args, .. } => args.clone(),
                _ => Vec::new(),
            },
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
        tools: config
            .tools
            .iter()
            .map(|(name, config)| (name.clone(), tool_specific_config_to_proto(config)))
            .collect(),
    }
}

pub fn tool_specific_config_to_proto(config: &ToolSpecificConfig) -> proto::ToolSpecificConfig {
    match config {
        ToolSpecificConfig::Bash(bash_config) => proto::ToolSpecificConfig {
            config: Some(proto::tool_specific_config::Config::Bash(
                proto::BashToolConfig {
                    approved_patterns: bash_config.approved_patterns.clone(),
                },
            )),
        },
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
        Some(proto::workspace_config::Config::Local(local)) => WorkspaceConfig::Local {
            path: if local.path.is_empty() {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            } else {
                std::path::PathBuf::from(local.path)
            },
        },
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
        None => WorkspaceConfig::Local {
            path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        },
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
                Some(proto::backend_config::Backend::Mcp(mcp)) => {
                    // Try to deserialize transport from JSON string
                    let transport = if !mcp.transport.is_empty() && mcp.transport != "{}" {
                        serde_json::from_str(&mcp.transport).unwrap_or_else(|_| {
                            // Fallback to stdio transport using command/args from proto
                            steer_core::tools::McpTransport::Stdio {
                                command: mcp.command.clone(),
                                args: mcp.args.clone(),
                            }
                        })
                    } else {
                        // Fallback for old proto format
                        steer_core::tools::McpTransport::Stdio {
                            command: mcp.command.clone(),
                            args: mcp.args.clone(),
                        }
                    };

                    BackendConfig::Mcp {
                        server_name: mcp.server_name,
                        transport,
                        tool_filter: proto_to_tool_filter(mcp.tool_filter),
                    }
                }
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
        tools: proto_config
            .tools
            .into_iter()
            .filter_map(|(name, config)| {
                proto_to_tool_specific_config(config).map(|config| (name, config))
            })
            .collect(),
    }
}

pub fn proto_to_tool_specific_config(
    proto_config: proto::ToolSpecificConfig,
) -> Option<ToolSpecificConfig> {
    match proto_config.config? {
        proto::tool_specific_config::Config::Bash(bash_config) => {
            Some(ToolSpecificConfig::Bash(BashToolConfig {
                approved_patterns: bash_config.approved_patterns,
            }))
        }
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
    use steer_core::app::conversation::{AssistantContent, ThoughtContent, UserContent};
    use steer_proto::agent::v1::{assistant_content, message, thought_content, user_content};

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
                            use steer_core::app::conversation::{
                                CommandResponse as AppCmdResponse,
                                CompactResult,
                            };
                            use steer_proto::agent::v1::{app_command_type, command_response};

                            let command = app_cmd.command.as_ref().and_then(|cmd| {
                                cmd.command_type.as_ref().map(|ct| match ct {
                                    app_command_type::CommandType::Model(model) => {
                                        AppCommandType::Model {
                                            target: model.target.clone(),
                                        }
                                    }
                                    app_command_type::CommandType::Clear(_) => AppCommandType::Clear,
                                    app_command_type::CommandType::Compact(_) => AppCommandType::Compact,
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
                                                proto::compact_result::ResultType::Success(summary) => {
                                                    CompactResult::Success(summary.clone())
                                                }
                                                proto::compact_result::ResultType::Cancelled(_) => {
                                                    CompactResult::Cancelled
                                                }
                                                proto::compact_result::ResultType::InsufficientMessages(_) => {
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
            Ok(ConversationMessage {
                data: MessageData::User { content },
                timestamp: user_msg.timestamp,
                id: proto_msg.id,
                parent_message_id: user_msg.parent_message_id,
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
                            let thought_content = thought.thought_type.as_ref().map(|t| match t {
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
            Ok(ConversationMessage {
                data: MessageData::Assistant { content },
                timestamp: assistant_msg.timestamp,
                id: proto_msg.id,
                parent_message_id: assistant_msg.parent_message_id,
            })
        }
        message::Message::Tool(tool_msg) => {
            if let Some(proto_result) = tool_msg.result {
                let tool_result = proto_to_steer_tools_result(proto_result)?;
                Ok(ConversationMessage {
                    data: MessageData::Tool {
                        tool_use_id: tool_msg.tool_use_id,
                        result: tool_result,
                    },
                    timestamp: tool_msg.timestamp,
                    id: proto_msg.id,
                    parent_message_id: tool_msg.parent_message_id,
                })
            } else {
                Err(ConversionError::MissingField {
                    field: "tool_msg.result".to_string(),
                })
            }
        }
    }
}

/// Convert AppEvent to protobuf StreamSessionResponse
pub fn app_event_to_server_event(
    app_event: AppEvent,
    sequence_num: u64,
) -> Result<proto::StreamSessionResponse, ConversionError> {
    let timestamp = Some(prost_types::Timestamp::from(std::time::SystemTime::now()));

    let event = match app_event {
        AppEvent::MessageAdded { message, model } => {
            let proto_message = message_to_proto(message);
            Some(proto::stream_session_response::Event::MessageAdded(
                proto::MessageAddedEvent {
                    message: Some(proto_message?),
                    model: model.to_string(),
                },
            ))
        }
        AppEvent::MessageUpdated { id, content } => {
            Some(proto::stream_session_response::Event::MessageUpdated(
                proto::MessageUpdatedEvent { id, content },
            ))
        }
        AppEvent::MessagePart { id, delta } => {
            Some(proto::stream_session_response::Event::MessagePart(
                proto::MessagePartEvent { id, delta },
            ))
        }
        AppEvent::ProcessingStarted => {
            Some(proto::stream_session_response::Event::ProcessingStarted(
                proto::ProcessingStartedEvent {},
            ))
        }
        AppEvent::ProcessingCompleted => {
            Some(proto::stream_session_response::Event::ProcessingCompleted(
                proto::ProcessingCompletedEvent {},
            ))
        }
        AppEvent::ToolCallStarted {
            name,
            id,
            parameters,
            model,
        } => Some(proto::stream_session_response::Event::ToolCallStarted(
            proto::ToolCallStartedEvent {
                name,
                id,
                model: model.to_string(),
                parameters_json: serde_json::to_string(&parameters).unwrap_or_default(),
            },
        )),
        AppEvent::ToolCallCompleted {
            name,
            result,
            id,
            model,
        } => {
            let proto_result = steer_tools_result_to_proto(&result)
                .map_err(|e| ConversionError::ToolResultConversion(e.to_string()))?;
            Some(proto::stream_session_response::Event::ToolCallCompleted(
                proto::ToolCallCompletedEvent {
                    name,
                    result: Some(proto_result),
                    id,
                    model: model.to_string(),
                },
            ))
        }
        AppEvent::ToolCallFailed {
            name,
            error,
            id,
            model,
        } => Some(proto::stream_session_response::Event::ToolCallFailed(
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
        } => Some(proto::stream_session_response::Event::RequestToolApproval(
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

            Some(proto::stream_session_response::Event::CommandResponse(
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
        AppEvent::ModelChanged { model } => Some(
            proto::stream_session_response::Event::ModelChanged(proto::ModelChangedEvent {
                model: model.to_string(),
            }),
        ),
        AppEvent::Error { message } => Some(proto::stream_session_response::Event::Error(
            proto::ErrorEvent { message },
        )),
        AppEvent::OperationCancelled { op_id: _, info } => {
            Some(proto::stream_session_response::Event::OperationCancelled(
                proto::OperationCancelledEvent {
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
                },
            ))
        }
        AppEvent::WorkspaceChanged => {
            Some(proto::stream_session_response::Event::WorkspaceChanged(
                proto::WorkspaceChangedEvent {},
            ))
        }
        AppEvent::WorkspaceFiles { files } => Some(
            proto::stream_session_response::Event::WorkspaceFiles(proto::WorkspaceFilesEvent {
                files: files.clone(),
            }),
        ),
        AppEvent::Started { id, op } => {
            let proto_op = match op {
                Operation::Bash { cmd } => {
                    proto::started_operation::Operation::Bash(proto::BashOperation {
                        cmd: cmd.clone(),
                    })
                }
                Operation::Compact => {
                    proto::started_operation::Operation::Compact(proto::CompactOperation {})
                }
            };
            Some(proto::stream_session_response::Event::Started(
                proto::StartedEvent {
                    id: id.as_bytes().to_vec(),
                    op: Some(proto::StartedOperation {
                        operation: Some(proto_op),
                    }),
                },
            ))
        }
        AppEvent::Finished { id, outcome } => {
            let proto_outcome = match outcome {
                OperationOutcome::Bash { elapsed, result } => {
                    let error = result.as_ref().err().map(|e| proto::BashError {
                        exit_code: e.exit_code,
                        stderr: e.stderr.clone(),
                    });
                    proto::operation_outcome::Outcome::Bash(proto::BashOutcome {
                        elapsed_ms: elapsed.as_millis() as u64,
                        error,
                    })
                }
                OperationOutcome::Compact { elapsed, result } => {
                    let error = result.as_ref().err().map(|e| proto::CompactError {
                        message: e.message.clone(),
                    });
                    proto::operation_outcome::Outcome::Compact(proto::CompactOutcome {
                        elapsed_ms: elapsed.as_millis() as u64,
                        error,
                    })
                }
            };
            Some(proto::stream_session_response::Event::Finished(
                proto::FinishedEvent {
                    id: id.as_bytes().to_vec(),
                    outcome: Some(proto::OperationOutcome {
                        outcome: Some(proto_outcome),
                    }),
                },
            ))
        }
        AppEvent::ActiveMessageIdChanged { message_id } => Some(
            proto::stream_session_response::Event::ActiveMessageIdChanged(
                proto::ActiveMessageIdChangedEvent { message_id },
            ),
        ),
    };

    Ok(proto::StreamSessionResponse {
        sequence_num,
        timestamp,
        event,
    })
}

fn proto_app_command_type_to_app_command_type(
    proto_command_type: &proto::app_command_type::CommandType,
) -> AppCommandType {
    match proto_command_type {
        proto::app_command_type::CommandType::Model(model) => AppCommandType::Model {
            target: model.target.clone(),
        },
        proto::app_command_type::CommandType::Clear(_) => AppCommandType::Clear,
        proto::app_command_type::CommandType::Compact(_) => AppCommandType::Compact,
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

/// Convert protobuf StreamSessionResponse to AppEvent
pub fn server_event_to_app_event(
    server_event: proto::StreamSessionResponse,
) -> Result<AppEvent, ConversionError> {
    use steer_core::app::cancellation::ActiveTool;
    let event = server_event
        .event
        .ok_or_else(|| ConversionError::MissingField {
            field: "server_event.event".to_string(),
        })?;

    match event {
        proto::stream_session_response::Event::MessageAdded(e) => {
            let proto_message = e.message.ok_or_else(|| ConversionError::MissingField {
                field: "message_added_event.message".to_string(),
            })?;

            let message = proto_to_message(proto_message)?;
            let model = {
                use std::str::FromStr;
                steer_core::api::Model::from_str(&e.model).map_err(|_| {
                    ConversionError::InvalidValue {
                        field: "model".to_string(),
                        value: e.model.clone(),
                    }
                })?
            };

            Ok(AppEvent::MessageAdded { message, model })
        }
        proto::stream_session_response::Event::MessageUpdated(e) => Ok(AppEvent::MessageUpdated {
            id: e.id,
            content: e.content,
        }),
        proto::stream_session_response::Event::MessagePart(e) => Ok(AppEvent::MessagePart {
            id: e.id,
            delta: e.delta,
        }),
        proto::stream_session_response::Event::ToolCallStarted(e) => {
            let model = {
                use std::str::FromStr;
                steer_core::api::Model::from_str(&e.model).map_err(|_| {
                    ConversionError::InvalidValue {
                        field: "model".to_string(),
                        value: e.model.clone(),
                    }
                })?
            };
            let parameters = serde_json::from_str(&e.parameters_json).map_err(|err| {
                ConversionError::InvalidJson {
                    field: "parameters_json".to_string(),
                    error: err.to_string(),
                }
            })?;
            Ok(AppEvent::ToolCallStarted {
                name: e.name,
                id: e.id,
                parameters,
                model,
            })
        }
        proto::stream_session_response::Event::ToolCallCompleted(e) => {
            let model = {
                use std::str::FromStr;
                steer_core::api::Model::from_str(&e.model).map_err(|_| {
                    ConversionError::InvalidValue {
                        field: "model".to_string(),
                        value: e.model.clone(),
                    }
                })?
            };
            Ok(AppEvent::ToolCallCompleted {
                name: e.name,
                result: proto_to_steer_tools_result(e.result.ok_or_else(|| {
                    ConversionError::MissingField {
                        field: "result".to_string(),
                    }
                })?)?,
                id: e.id,
                model,
            })
        }
        proto::stream_session_response::Event::ToolCallFailed(e) => {
            let model = {
                use std::str::FromStr;
                steer_core::api::Model::from_str(&e.model).map_err(|_| {
                    ConversionError::InvalidValue {
                        field: "model".to_string(),
                        value: e.model.clone(),
                    }
                })?
            };
            Ok(AppEvent::ToolCallFailed {
                name: e.name,
                error: e.error,
                id: e.id,
                model,
            })
        }
        proto::stream_session_response::Event::ProcessingStarted(_) => {
            Ok(AppEvent::ProcessingStarted)
        }
        proto::stream_session_response::Event::ProcessingCompleted(_) => {
            Ok(AppEvent::ProcessingCompleted)
        }
        proto::stream_session_response::Event::RequestToolApproval(e) => {
            let parameters = serde_json::from_str(&e.parameters_json).map_err(|err| {
                ConversionError::InvalidJson {
                    field: "parameters_json".to_string(),
                    error: err.to_string(),
                }
            })?;
            Ok(AppEvent::RequestToolApproval {
                name: e.name,
                parameters,
                id: e.id,
            })
        }
        proto::stream_session_response::Event::OperationCancelled(e) => {
            let info = e.info.ok_or_else(|| ConversionError::MissingField {
                field: "operation_cancelled.info".to_string(),
            })?;

            Ok(AppEvent::OperationCancelled {
                op_id: None, // Operation ID not available in old proto format
                info: steer_core::app::cancellation::CancellationInfo {
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
        }
        proto::stream_session_response::Event::CommandResponse(e) => {
            // Convert proto command to app command
            let command = e
                .command
                .as_ref()
                .and_then(|cmd| {
                    cmd.command_type
                        .as_ref()
                        .map(proto_app_command_type_to_app_command_type)
                })
                .ok_or_else(|| ConversionError::MissingField {
                    field: "command_response.command".to_string(),
                })?;

            // Convert proto response to app response
            let response = e
                .response
                .as_ref()
                .and_then(|resp| {
                    resp.response
                        .as_ref()
                        .map(proto_command_response_response_to_command_response)
                })
                .ok_or_else(|| ConversionError::MissingField {
                    field: "command_response.response".to_string(),
                })?;

            Ok(AppEvent::CommandResponse {
                command,
                response,
                id: e.id,
            })
        }
        proto::stream_session_response::Event::ModelChanged(e) => {
            let model = {
                use std::str::FromStr;
                steer_core::api::Model::from_str(&e.model).map_err(|_| {
                    ConversionError::InvalidValue {
                        field: "model".to_string(),
                        value: e.model.clone(),
                    }
                })?
            };
            Ok(AppEvent::ModelChanged { model })
        }
        proto::stream_session_response::Event::Error(e) => {
            Ok(AppEvent::Error { message: e.message })
        }
        proto::stream_session_response::Event::WorkspaceChanged(_) => {
            Ok(AppEvent::WorkspaceChanged)
        }
        proto::stream_session_response::Event::WorkspaceFiles(e) => Ok(AppEvent::WorkspaceFiles {
            files: e.files.clone(),
        }),
        proto::stream_session_response::Event::Started(e) => {
            let id = uuid::Uuid::from_slice(&e.id).map_err(|_| ConversionError::InvalidValue {
                field: "started.id".to_string(),
                value: format!("{:?}", e.id),
            })?;

            let op = e.op.as_ref().ok_or_else(|| ConversionError::MissingField {
                field: "started.op".to_string(),
            })?;

            let operation = match &op.operation {
                Some(proto::started_operation::Operation::Bash(b)) => {
                    Operation::Bash { cmd: b.cmd.clone() }
                }
                Some(proto::started_operation::Operation::Compact(_)) => Operation::Compact,
                None => {
                    return Err(ConversionError::MissingField {
                        field: "started.op.operation".to_string(),
                    });
                }
            };

            Ok(AppEvent::Started { id, op: operation })
        }
        proto::stream_session_response::Event::Finished(e) => {
            let id = uuid::Uuid::from_slice(&e.id).map_err(|_| ConversionError::InvalidValue {
                field: "finished.id".to_string(),
                value: format!("{:?}", e.id),
            })?;

            let outcome_proto =
                e.outcome
                    .as_ref()
                    .ok_or_else(|| ConversionError::MissingField {
                        field: "finished.outcome".to_string(),
                    })?;

            let outcome = match &outcome_proto.outcome {
                Some(proto::operation_outcome::Outcome::Bash(b)) => {
                    let result = if let Some(error) = &b.error {
                        Err(BashError {
                            exit_code: error.exit_code,
                            stderr: error.stderr.clone(),
                        })
                    } else {
                        Ok(())
                    };
                    OperationOutcome::Bash {
                        elapsed: Duration::from_millis(b.elapsed_ms),
                        result,
                    }
                }
                Some(proto::operation_outcome::Outcome::Compact(c)) => {
                    let result = if let Some(error) = &c.error {
                        Err(CompactError {
                            message: error.message.clone(),
                        })
                    } else {
                        Ok(())
                    };
                    OperationOutcome::Compact {
                        elapsed: Duration::from_millis(c.elapsed_ms),
                        result,
                    }
                }
                None => {
                    return Err(ConversionError::MissingField {
                        field: "finished.outcome.outcome".to_string(),
                    });
                }
            };

            Ok(AppEvent::Finished { id, outcome })
        }
        proto::stream_session_response::Event::ActiveMessageIdChanged(e) => {
            Ok(AppEvent::ActiveMessageIdChanged {
                message_id: e.message_id,
            })
        }
    }
}

/// Convert TUI AppCommand to gRPC StreamSessionRequest
pub fn convert_app_command_to_client_message(
    command: AppCommand,
    session_id: &str,
) -> Result<Option<proto::StreamSessionRequest>, ConversionError> {
    use proto::stream_session_request::Message as StreamSessionRequestType;

    let message = match command {
        AppCommand::ProcessUserInput(text) => Some(StreamSessionRequestType::SendMessage(
            proto::SendMessageRequest {
                session_id: session_id.to_string(),
                message: text,
                attachments: vec![],
            },
        )),

        AppCommand::EditMessage {
            message_id,
            new_content,
        } => Some(StreamSessionRequestType::EditMessage(
            proto::EditMessageRequest {
                session_id: session_id.to_string(),
                message_id,
                new_content,
            },
        )),

        AppCommand::HandleToolResponse { id, approval } => {
            let decision = match approval {
                ApprovalType::Denied => proto::ApprovalDecision {
                    decision_type: Some(proto::approval_decision::DecisionType::Deny(true)),
                },
                ApprovalType::Once => proto::ApprovalDecision {
                    decision_type: Some(proto::approval_decision::DecisionType::Once(true)),
                },
                ApprovalType::AlwaysTool => proto::ApprovalDecision {
                    decision_type: Some(proto::approval_decision::DecisionType::AlwaysTool(true)),
                },
                ApprovalType::AlwaysBashPattern(pattern) => proto::ApprovalDecision {
                    decision_type: Some(proto::approval_decision::DecisionType::AlwaysBashPattern(
                        pattern,
                    )),
                },
            };

            Some(StreamSessionRequestType::ToolApproval(
                proto::ToolApprovalResponse {
                    tool_call_id: id,
                    decision: Some(decision),
                },
            ))
        }

        AppCommand::CancelProcessing => {
            Some(StreamSessionRequestType::Cancel(
                proto::CancelOperationRequest {
                    session_id: session_id.to_string(),
                    operation_id: String::new(), // Server will cancel current operation
                },
            ))
        }

        // ExecuteCommand and ExecuteBashCommand map to specific gRPC messages
        AppCommand::ExecuteCommand(command) => Some(StreamSessionRequestType::ExecuteCommand(
            proto::ExecuteCommandRequest {
                session_id: session_id.to_string(),
                command: command.as_command_str(),
            },
        )),
        AppCommand::ExecuteBashCommand { command } => Some(
            StreamSessionRequestType::ExecuteBashCommand(proto::ExecuteBashCommandRequest {
                session_id: session_id.to_string(),
                command,
            }),
        ),

        // These commands don't map to gRPC messages
        AppCommand::Shutdown => None,
        AppCommand::RequestToolApprovalInternal { .. } => None,
        AppCommand::RestoreConversation { .. } => None,
        AppCommand::GetCurrentConversation => None,
        AppCommand::RequestWorkspaceFiles => None,
    };

    Ok(message.map(|msg| proto::StreamSessionRequest {
        session_id: session_id.to_string(),
        message: Some(msg),
    }))
}

pub fn convert_todo_item_to_proto(item: &TodoItem) -> common::TodoItem {
    common::TodoItem {
        id: item.id.clone(),
        content: item.content.clone(),
        status: match item.status {
            TodoStatus::Pending => common::TodoStatus::Pending as i32,
            TodoStatus::InProgress => common::TodoStatus::InProgress as i32,
            TodoStatus::Completed => common::TodoStatus::Completed as i32,
        },
        priority: match item.priority {
            TodoPriority::High => common::TodoPriority::High as i32,
            TodoPriority::Medium => common::TodoPriority::Medium as i32,
            TodoPriority::Low => common::TodoPriority::Low as i32,
        },
    }
}

pub fn convert_proto_to_todo_item(item: common::TodoItem) -> TodoItem {
    TodoItem {
        id: item.id.clone(),
        content: item.content.clone(),
        status: match steer_proto::common::v1::TodoStatus::try_from(item.status) {
            Ok(steer_proto::common::v1::TodoStatus::Pending) => TodoStatus::Pending,
            Ok(steer_proto::common::v1::TodoStatus::InProgress) => TodoStatus::InProgress,
            Ok(steer_proto::common::v1::TodoStatus::Completed) => TodoStatus::Completed,
            Ok(steer_proto::common::v1::TodoStatus::StatusUnset) => TodoStatus::Pending,
            Err(_) => TodoStatus::Pending,
        },
        priority: match steer_proto::common::v1::TodoPriority::try_from(item.priority) {
            Ok(steer_proto::common::v1::TodoPriority::High) => TodoPriority::High,
            Ok(steer_proto::common::v1::TodoPriority::Medium) => TodoPriority::Medium,
            Ok(steer_proto::common::v1::TodoPriority::Low) => TodoPriority::Low,
            Ok(steer_proto::common::v1::TodoPriority::PriorityUnset) => TodoPriority::Low,
            Err(_) => TodoPriority::Low,
        },
    }
}

pub fn convert_proto_to_todo_write_file_operation(
    operation: common::TodoWriteFileOperation,
) -> TodoWriteFileOperation {
    match operation {
        common::TodoWriteFileOperation::Created => TodoWriteFileOperation::Created,
        common::TodoWriteFileOperation::Modified => TodoWriteFileOperation::Modified,
        common::TodoWriteFileOperation::OperationUnset => TodoWriteFileOperation::Created,
    }
}

pub fn convert_todo_write_file_operation_to_proto(
    operation: &TodoWriteFileOperation,
) -> common::TodoWriteFileOperation {
    match operation {
        TodoWriteFileOperation::Created => common::TodoWriteFileOperation::Created,
        TodoWriteFileOperation::Modified => common::TodoWriteFileOperation::Modified,
    }
}
