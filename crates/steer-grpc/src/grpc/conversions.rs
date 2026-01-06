use crate::grpc::error::ConversionError;
use chrono::{DateTime, Utc};
use steer_core::app::conversation::{
    AssistantContent, Message as ConversationMessage, MessageData, ThoughtContent, UserContent,
};
use steer_core::app::domain::SessionEvent;

use steer_core::session::state::{
    BackendConfig, BashToolConfig, RemoteAuth, SessionConfig, SessionToolConfig,
    ToolApprovalPolicy, ToolFilter, ToolSpecificConfig, ToolVisibility, WorkspaceConfig,
};
use steer_proto::agent::v1 as proto;
use steer_proto::common::v1 as common;
use steer_tools::ToolCall;
use steer_tools::tools::todo::{TodoItem, TodoPriority, TodoStatus, TodoWriteFileOperation};

/// Convert a core ModelId to proto ModelSpec
fn model_to_proto(model: steer_core::config::model::ModelId) -> proto::ModelSpec {
    proto::ModelSpec {
        provider_id: model.0.storage_key(),
        model_id: model.1.clone(),
    }
}

/// Convert proto ModelSpec to core ModelId
fn proto_to_model(
    spec: &proto::ModelSpec,
) -> Result<steer_core::config::model::ModelId, ConversionError> {
    use steer_core::config::provider::ProviderId;

    // Use serde to deserialize the provider ID
    let provider_id: ProviderId = serde_json::from_value(serde_json::json!(&spec.provider_id))
        .map_err(|e| ConversionError::InvalidData {
            message: format!("Invalid provider ID '{}': {}", spec.provider_id, e),
        })?;

    Ok((provider_id, spec.model_id.clone()))
}
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
    })
}

pub(crate) fn message_to_proto(
    message: ConversationMessage,
) -> Result<proto::Message, ConversionError> {
    let (message_variant, created_at) = match &message.data {
        MessageData::User { content, .. } => {
            let user_msg = proto::UserMessage {
                content: content
                    .iter()
                    .filter_map(|user_content| match user_content {
                        UserContent::Text { text } => Some(proto::UserContent {
                            content: Some(proto::user_content::Content::Text(text.clone())),
                        }),
                        UserContent::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        } => Some(proto::UserContent {
                            content: Some(proto::user_content::Content::CommandExecution(
                                proto::CommandExecution {
                                    command: command.clone(),
                                    stdout: stdout.clone(),
                                    stderr: stderr.clone(),
                                    exit_code: *exit_code,
                                },
                            )),
                        }),
                        UserContent::AppCommand { .. } => {
                            // AppCommand is no longer serialized over the wire
                            None
                        }
                    })
                    .collect(),
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
pub(crate) fn tool_approval_policy_to_proto(
    policy: &ToolApprovalPolicy,
) -> proto::ToolApprovalPolicy {
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
pub(crate) fn workspace_config_to_proto(config: &WorkspaceConfig) -> proto::WorkspaceConfig {
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
    };

    proto::WorkspaceConfig {
        config: Some(config_variant),
    }
}

/// Convert internal RemoteAuth to protobuf
pub(crate) fn remote_auth_to_proto(auth: &RemoteAuth) -> proto::RemoteAuth {
    use proto::remote_auth::Auth;

    let auth_variant = match auth {
        RemoteAuth::Bearer { token } => Auth::BearerToken(token.clone()),
        RemoteAuth::ApiKey { key } => Auth::ApiKey(key.clone()),
    };

    proto::RemoteAuth {
        auth: Some(auth_variant),
    }
}

/// Convert internal ToolFilter to protobuf
pub(crate) fn tool_filter_to_proto(filter: &ToolFilter) -> proto::ToolFilter {
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
pub(crate) fn tool_visibility_to_proto(visibility: &ToolVisibility) -> proto::ToolVisibility {
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
pub(crate) fn backend_config_to_proto(config: &BackendConfig) -> proto::BackendConfig {
    use proto::backend_config::Backend;

    let BackendConfig::Mcp {
        server_name,
        transport,
        tool_filter,
    } = config;

    let backend_variant = Backend::Mcp(proto::McpBackendConfig {
        server_name: server_name.clone(),
        transport: serde_json::to_string(&transport).unwrap_or_else(|_| "{}".to_string()),
        command: match transport {
            steer_core::tools::McpTransport::Stdio { command, .. } => command.clone(),
            _ => String::new(),
        },
        args: match transport {
            steer_core::tools::McpTransport::Stdio { args, .. } => args.clone(),
            _ => Vec::new(),
        },
        tool_filter: Some(tool_filter_to_proto(tool_filter)),
    });

    proto::BackendConfig {
        backend: Some(backend_variant),
    }
}

/// Convert internal SessionToolConfig to protobuf
pub(crate) fn session_tool_config_to_proto(config: &SessionToolConfig) -> proto::SessionToolConfig {
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

pub(crate) fn tool_specific_config_to_proto(
    config: &ToolSpecificConfig,
) -> proto::ToolSpecificConfig {
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
pub(crate) fn session_config_to_proto(config: &SessionConfig) -> proto::SessionConfig {
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
pub(crate) fn proto_to_workspace_config(proto_config: proto::WorkspaceConfig) -> WorkspaceConfig {
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
        None => WorkspaceConfig::Local {
            path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        },
    }
}

/// Convert from protobuf ToolFilter to internal ToolFilter
pub(crate) fn proto_to_tool_filter(proto_filter: Option<proto::ToolFilter>) -> ToolFilter {
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
pub(crate) fn proto_to_tool_visibility(
    proto_visibility: Option<proto::ToolVisibility>,
) -> ToolVisibility {
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
pub(crate) fn proto_to_tool_approval_policy(
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
pub(crate) fn proto_to_tool_config(proto_config: proto::SessionToolConfig) -> SessionToolConfig {
    let backends = proto_config
        .backends
        .into_iter()
        .filter_map(|proto_backend| match proto_backend.backend {
            Some(proto::backend_config::Backend::Local(_)) => None,
            Some(proto::backend_config::Backend::Mcp(mcp)) => {
                let transport = if !mcp.transport.is_empty() && mcp.transport != "{}" {
                    serde_json::from_str(&mcp.transport).unwrap_or_else(|_| {
                        steer_core::tools::McpTransport::Stdio {
                            command: mcp.command.clone(),
                            args: mcp.args.clone(),
                        }
                    })
                } else {
                    steer_core::tools::McpTransport::Stdio {
                        command: mcp.command.clone(),
                        args: mcp.args.clone(),
                    }
                };

                Some(BackendConfig::Mcp {
                    server_name: mcp.server_name,
                    transport,
                    tool_filter: proto_to_tool_filter(mcp.tool_filter),
                })
            }
            None => None,
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

pub(crate) fn proto_to_tool_specific_config(
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
pub(crate) fn proto_tool_call_to_core(
    proto_tool_call: &proto::ToolCall,
) -> Result<ToolCall, ConversionError> {
    let parameters = serde_json::from_str(&proto_tool_call.parameters_json)?;
    Ok(ToolCall {
        id: proto_tool_call.id.clone(),
        name: proto_tool_call.name.clone(),
        parameters,
    })
}

pub(crate) fn proto_to_message(
    proto_msg: proto::Message,
) -> Result<ConversationMessage, ConversionError> {
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

fn compaction_record_to_proto(
    record: &steer_core::app::domain::types::CompactionRecord,
) -> proto::CompactionRecord {
    proto::CompactionRecord {
        id: record.id.to_string(),
        summary_message_id: record.summary_message_id.to_string(),
        compacted_head_message_id: record.compacted_head_message_id.to_string(),
        previous_active_message_id: record
            .previous_active_message_id
            .as_ref()
            .map(|id| id.to_string()),
        model: record.model.clone(),
        created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::from(
            record.created_at,
        ))),
    }
}

/// Convert domain SessionEvent to protobuf SessionEvent
///
/// This is used by the new RuntimeService architecture to convert events
/// from the pure reducer-based domain model to the gRPC protocol.
pub(crate) fn session_event_to_proto(
    session_event: SessionEvent,
    sequence_num: u64,
    current_model: &steer_core::config::model::ModelId,
) -> Result<proto::SessionEvent, ConversionError> {
    let timestamp = Some(prost_types::Timestamp::from(std::time::SystemTime::now()));

    let event = match session_event {
        SessionEvent::SessionCreated { .. } => None,
        SessionEvent::MessageAdded { message, model } => {
            let proto_message = message_to_proto(message)?;
            Some(proto::session_event::Event::MessageAdded(
                proto::MessageAddedEvent {
                    message: Some(proto_message),
                    model: Some(model_to_proto(model)),
                },
            ))
        }
        SessionEvent::MessageUpdated { id, content } => Some(
            proto::session_event::Event::MessageUpdated(proto::MessageUpdatedEvent {
                id: id.to_string(),
                content,
            }),
        ),
        SessionEvent::ToolCallStarted {
            id,
            name,
            parameters,
        } => Some(proto::session_event::Event::ToolCallStarted(
            proto::ToolCallStartedEvent {
                name,
                id: id.to_string(),
                model: Some(model_to_proto(current_model.clone())),
                parameters_json: serde_json::to_string(&parameters).unwrap_or_default(),
            },
        )),
        SessionEvent::ToolCallCompleted { id, name, result } => {
            let proto_result = steer_tools_result_to_proto(&result)
                .map_err(|e| ConversionError::ToolResultConversion(e.to_string()))?;
            Some(proto::session_event::Event::ToolCallCompleted(
                proto::ToolCallCompletedEvent {
                    name,
                    result: Some(proto_result),
                    id: id.to_string(),
                    model: Some(model_to_proto(current_model.clone())),
                },
            ))
        }
        SessionEvent::ToolCallFailed { id, name, error } => Some(
            proto::session_event::Event::ToolCallFailed(proto::ToolCallFailedEvent {
                name,
                error,
                id: id.to_string(),
                model: Some(model_to_proto(current_model.clone())),
            }),
        ),
        SessionEvent::ApprovalRequested {
            request_id,
            tool_call,
        } => Some(proto::session_event::Event::RequestToolApproval(
            proto::RequestToolApprovalEvent {
                name: tool_call.name.clone(),
                parameters_json: serde_json::to_string(&tool_call.parameters).unwrap_or_default(),
                id: request_id.to_string(),
            },
        )),
        SessionEvent::ApprovalDecided { .. } => {
            // This is an internal state change, not typically sent to clients
            // The client already knows about their own approval decision
            None
        }
        SessionEvent::OperationStarted { op_id, kind: _ } => Some(
            proto::session_event::Event::ProcessingStarted(proto::ProcessingStartedEvent {
                op_id: op_id.to_string(),
            }),
        ),
        SessionEvent::OperationCompleted { op_id } => Some(
            proto::session_event::Event::ProcessingCompleted(proto::ProcessingCompletedEvent {
                op_id: op_id.to_string(),
            }),
        ),
        SessionEvent::OperationCancelled { op_id, info } => Some(
            proto::session_event::Event::OperationCancelled(proto::OperationCancelledEvent {
                op_id: op_id.to_string(),
                info: Some(proto::CancellationInfo {
                    api_call_in_progress: false,
                    active_tools: vec![],
                    pending_tool_approvals: info.pending_tool_calls > 0,
                }),
            }),
        ),
        SessionEvent::ModelChanged { model } => Some(proto::session_event::Event::ModelChanged(
            proto::ModelChangedEvent {
                model: Some(model_to_proto(model)),
            },
        )),
        SessionEvent::ConversationCompacted { record } => Some(
            proto::session_event::Event::ConversationCompacted(proto::ConversationCompactedEvent {
                record: Some(compaction_record_to_proto(&record)),
            }),
        ),
        SessionEvent::WorkspaceChanged => Some(proto::session_event::Event::WorkspaceChanged(
            proto::WorkspaceChangedEvent {},
        )),
        SessionEvent::Error { message } => {
            Some(proto::session_event::Event::Error(proto::ErrorEvent {
                message,
            }))
        }
    };

    Ok(proto::SessionEvent {
        sequence_num,
        timestamp,
        event,
    })
}

// TODO: These todo conversion functions are pub because steer-remote-workspace uses them.
// This is a boundary violation - should be refactored to use shared proto conversion traits.
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

pub(crate) fn convert_proto_to_todo_item(item: common::TodoItem) -> TodoItem {
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

pub(crate) fn convert_proto_to_todo_write_file_operation(
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

pub(crate) fn mcp_transport_to_proto(
    transport: &steer_core::tools::McpTransport,
) -> proto::McpTransportInfo {
    use steer_core::tools::McpTransport;

    proto::McpTransportInfo {
        transport: Some(match transport {
            McpTransport::Stdio { command, args } => {
                proto::mcp_transport_info::Transport::Stdio(proto::McpStdioTransport {
                    command: command.clone(),
                    args: args.clone(),
                })
            }
            McpTransport::Tcp { host, port } => {
                proto::mcp_transport_info::Transport::Tcp(proto::McpTcpTransport {
                    host: host.clone(),
                    port: *port as u32,
                })
            }
            McpTransport::Unix { path } => {
                proto::mcp_transport_info::Transport::Unix(proto::McpUnixTransport {
                    path: path.clone(),
                })
            }
            McpTransport::Sse { url, headers } => {
                proto::mcp_transport_info::Transport::Sse(proto::McpSseTransport {
                    url: url.clone(),
                    headers: headers.clone().unwrap_or_default(),
                })
            }
            McpTransport::Http { url, headers } => {
                proto::mcp_transport_info::Transport::Http(proto::McpHttpTransport {
                    url: url.clone(),
                    headers: headers.clone().unwrap_or_default(),
                })
            }
        }),
    }
}

pub(crate) fn proto_to_mcp_server_info(
    proto: proto::McpServerInfo,
) -> Result<steer_core::session::state::McpServerInfo, ConversionError> {
    use steer_core::session::state::McpConnectionState;

    let transport = proto
        .transport
        .ok_or_else(|| ConversionError::MissingField {
            field: "transport".to_string(),
        })?;
    let transport = proto_to_mcp_transport(transport)?;

    let state = proto.state.ok_or_else(|| ConversionError::MissingField {
        field: "state".to_string(),
    })?;
    let state = match state.state {
        Some(proto::mcp_connection_state::State::Connecting(_)) => McpConnectionState::Connecting,
        Some(proto::mcp_connection_state::State::Connected(connected)) => {
            McpConnectionState::Connected {
                tool_names: connected.tool_names,
            }
        }
        Some(proto::mcp_connection_state::State::Failed(failed)) => McpConnectionState::Failed {
            error: failed.error,
        },
        None => {
            return Err(ConversionError::MissingField {
                field: "state.state".to_string(),
            });
        }
    };

    let last_updated = proto
        .last_updated
        .ok_or_else(|| ConversionError::MissingField {
            field: "last_updated".to_string(),
        })?;
    let last_updated =
        DateTime::<Utc>::from_timestamp(last_updated.seconds, last_updated.nanos as u32)
            .ok_or_else(|| ConversionError::InvalidData {
                message: "Invalid timestamp".to_string(),
            })?;

    Ok(steer_core::session::state::McpServerInfo {
        server_name: proto.server_name,
        transport,
        state,
        last_updated,
    })
}

fn proto_to_mcp_transport(
    proto: proto::McpTransportInfo,
) -> Result<steer_core::tools::McpTransport, ConversionError> {
    use steer_core::tools::McpTransport;

    let transport = proto
        .transport
        .ok_or_else(|| ConversionError::MissingField {
            field: "transport".to_string(),
        })?;

    Ok(match transport {
        proto::mcp_transport_info::Transport::Stdio(stdio) => McpTransport::Stdio {
            command: stdio.command,
            args: stdio.args,
        },
        proto::mcp_transport_info::Transport::Tcp(tcp) => McpTransport::Tcp {
            host: tcp.host,
            port: tcp.port as u16,
        },
        proto::mcp_transport_info::Transport::Unix(unix) => McpTransport::Unix { path: unix.path },
        proto::mcp_transport_info::Transport::Sse(sse) => McpTransport::Sse {
            url: sse.url,
            headers: if sse.headers.is_empty() {
                None
            } else {
                Some(sse.headers)
            },
        },
        proto::mcp_transport_info::Transport::Http(http) => McpTransport::Http {
            url: http.url,
            headers: if http.headers.is_empty() {
                None
            } else {
                Some(http.headers)
            },
        },
    })
}

fn parse_op_id(s: &str) -> Result<crate::client_api::OpId, ConversionError> {
    uuid::Uuid::parse_str(s)
        .map(crate::client_api::OpId::from)
        .map_err(|_| ConversionError::InvalidData {
            message: format!("Invalid op_id: {}", s),
        })
}

fn parse_request_id(s: &str) -> Result<crate::client_api::RequestId, ConversionError> {
    uuid::Uuid::parse_str(s)
        .map(crate::client_api::RequestId::from)
        .map_err(|_| ConversionError::InvalidData {
            message: format!("Invalid request_id: {}", s),
        })
}

pub(crate) fn proto_to_client_event(
    server_event: proto::SessionEvent,
) -> Result<Option<crate::client_api::ClientEvent>, ConversionError> {
    use crate::client_api::{ClientEvent, MessageId, ToolCallId};
    use steer_tools::ToolCall;

    let event = match server_event.event {
        None => return Ok(None),
        Some(e) => e,
    };

    let client_event = match event {
        proto::session_event::Event::MessageAdded(e) => {
            let proto_message = e.message.ok_or_else(|| ConversionError::MissingField {
                field: "message_added_event.message".to_string(),
            })?;
            let message = proto_to_message(proto_message)?;
            let model = e
                .model
                .ok_or_else(|| ConversionError::MissingField {
                    field: "model".to_string(),
                })
                .and_then(|spec| proto_to_model(&spec))?;
            ClientEvent::MessageAdded { message, model }
        }
        proto::session_event::Event::MessageUpdated(e) => ClientEvent::MessageUpdated {
            id: MessageId::from(e.id),
            content: e.content,
        },
        proto::session_event::Event::MessagePart(e) => ClientEvent::MessageDelta {
            id: MessageId::from(e.id),
            delta: e.delta,
        },
        proto::session_event::Event::ToolCallStarted(e) => {
            let parameters = serde_json::from_str(&e.parameters_json).map_err(|err| {
                ConversionError::InvalidJson {
                    field: "parameters_json".to_string(),
                    error: err.to_string(),
                }
            })?;
            ClientEvent::ToolStarted {
                id: ToolCallId::from(e.id),
                name: e.name,
                parameters,
            }
        }
        proto::session_event::Event::ToolCallCompleted(e) => {
            let result = proto_to_steer_tools_result(e.result.ok_or_else(|| {
                ConversionError::MissingField {
                    field: "result".to_string(),
                }
            })?)?;
            ClientEvent::ToolCompleted {
                id: ToolCallId::from(e.id),
                name: e.name,
                result,
            }
        }
        proto::session_event::Event::ToolCallFailed(e) => ClientEvent::ToolFailed {
            id: ToolCallId::from(e.id),
            name: e.name,
            error: e.error,
        },
        proto::session_event::Event::ProcessingStarted(e) => {
            let op_id = parse_op_id(&e.op_id)?;
            ClientEvent::ProcessingStarted { op_id }
        }
        proto::session_event::Event::ProcessingCompleted(e) => {
            let op_id = parse_op_id(&e.op_id)?;
            ClientEvent::ProcessingCompleted { op_id }
        }
        proto::session_event::Event::RequestToolApproval(e) => {
            let parameters = serde_json::from_str(&e.parameters_json).map_err(|err| {
                ConversionError::InvalidJson {
                    field: "parameters_json".to_string(),
                    error: err.to_string(),
                }
            })?;
            let request_id = parse_request_id(&e.id)?;
            ClientEvent::ApprovalRequested {
                request_id,
                tool_call: ToolCall {
                    id: e.id,
                    name: e.name,
                    parameters,
                },
            }
        }
        proto::session_event::Event::OperationCancelled(e) => {
            let info = e.info.ok_or_else(|| ConversionError::MissingField {
                field: "operation_cancelled.info".to_string(),
            })?;
            let op_id = parse_op_id(&e.op_id)?;
            ClientEvent::OperationCancelled {
                op_id,
                pending_tool_calls: info.active_tools.len(),
            }
        }
        proto::session_event::Event::ModelChanged(e) => {
            let model = e
                .model
                .ok_or_else(|| ConversionError::MissingField {
                    field: "model".to_string(),
                })
                .and_then(|spec| proto_to_model(&spec))?;
            ClientEvent::ModelChanged { model }
        }
        proto::session_event::Event::Error(e) => ClientEvent::Error { message: e.message },
        proto::session_event::Event::WorkspaceChanged(_) => ClientEvent::WorkspaceChanged,
        proto::session_event::Event::Started(_)
        | proto::session_event::Event::Finished(_)
        | proto::session_event::Event::ActiveMessageIdChanged(_)
        | proto::session_event::Event::ConversationCompacted(_)
        | proto::session_event::Event::StreamDelta(_) => {
            return Ok(None);
        }
    };

    Ok(Some(client_event))
}
