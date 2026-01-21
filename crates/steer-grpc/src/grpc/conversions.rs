use crate::grpc::error::ConversionError;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use steer_core::app::conversation::{
    AssistantContent, Message as ConversationMessage, MessageData, ThoughtContent, UserContent,
};
use steer_core::app::domain::types::SessionId;
use steer_core::app::domain::{SessionEvent, StreamDelta, ToolCallDelta as CoreToolCallDelta};

use steer_core::session::state::{
    ApprovalRules, BackendConfig, RemoteAuth, SessionConfig, SessionToolConfig, ToolApprovalPolicy,
    ToolFilter, ToolRule, ToolVisibility, UnapprovedBehavior, WorkspaceConfig,
};
use steer_proto::agent::v1 as proto;
use steer_proto::common::v1 as common;
use steer_proto::remote_workspace::v1 as remote_proto;
use steer_tools::ToolCall;
use steer_tools::tools::todo::{TodoItem, TodoPriority, TodoStatus, TodoWriteFileOperation};
use uuid::Uuid;

/// Convert a core ModelId to proto ModelSpec
pub fn model_to_proto(model: steer_core::config::model::ModelId) -> proto::ModelSpec {
    proto::ModelSpec {
        provider_id: model.provider.storage_key(),
        model_id: model.id.clone(),
    }
}

/// Convert proto ModelSpec to core ModelId
pub fn proto_to_model(
    spec: &proto::ModelSpec,
) -> Result<steer_core::config::model::ModelId, ConversionError> {
    use steer_core::config::provider::ProviderId;

    // Use serde to deserialize the provider ID
    let provider_id: ProviderId = serde_json::from_value(serde_json::json!(&spec.provider_id))
        .map_err(|e| ConversionError::InvalidData {
            message: format!("Invalid provider ID '{}': {}", spec.provider_id, e),
        })?;

    Ok(steer_core::config::model::ModelId::new(
        provider_id,
        spec.model_id.clone(),
    ))
}

fn agent_workspace_revision_to_proto(
    revision: &steer_tools::result::AgentWorkspaceRevision,
) -> proto::AgentWorkspaceRevision {
    proto::AgentWorkspaceRevision {
        vcs_kind: revision.vcs_kind.clone(),
        revision_id: revision.revision_id.clone(),
        summary: revision.summary.clone(),
        change_id: revision.change_id.clone().unwrap_or_default(),
    }
}

fn agent_workspace_info_to_proto(
    info: &steer_tools::result::AgentWorkspaceInfo,
) -> proto::AgentWorkspaceInfo {
    proto::AgentWorkspaceInfo {
        workspace_id: info.workspace_id.clone().unwrap_or_default(),
        revision: info
            .revision
            .as_ref()
            .map(agent_workspace_revision_to_proto),
    }
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
            workspace: r.workspace.as_ref().map(agent_workspace_info_to_proto),
            session_id: r.session_id.clone().unwrap_or_default(),
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

fn proto_to_agent_workspace_revision(
    revision: proto::AgentWorkspaceRevision,
) -> steer_tools::result::AgentWorkspaceRevision {
    steer_tools::result::AgentWorkspaceRevision {
        vcs_kind: revision.vcs_kind,
        revision_id: revision.revision_id,
        summary: revision.summary,
        change_id: if revision.change_id.is_empty() {
            None
        } else {
            Some(revision.change_id)
        },
    }
}

fn proto_to_agent_workspace_info(
    info: proto::AgentWorkspaceInfo,
) -> steer_tools::result::AgentWorkspaceInfo {
    steer_tools::result::AgentWorkspaceInfo {
        workspace_id: if info.workspace_id.is_empty() {
            None
        } else {
            Some(info.workspace_id)
        },
        revision: info.revision.map(proto_to_agent_workspace_revision),
    }
}

/// Convert ToolError to protobuf
fn tool_error_to_proto(error: &steer_tools::error::ToolError) -> proto::ToolError {
    use proto::tool_error::ErrorType;
    use steer_tools::error::ToolError;

    let error_type = match error {
        ToolError::UnknownTool(name) => ErrorType::UnknownTool(name.clone()),
        ToolError::InvalidParams { tool_name, message } => {
            ErrorType::InvalidParams(proto::InvalidParamsError {
                tool_name: tool_name.clone(),
                message: message.clone(),
            })
        }
        ToolError::Execution(error) => ErrorType::Execution(proto::ExecutionError {
            tool_name: error.tool_name().to_string(),
            message: error.to_string(),
        }),
        ToolError::Cancelled(name) => ErrorType::Cancelled(name.clone()),
        ToolError::Timeout(name) => ErrorType::Timeout(name.clone()),
        ToolError::DeniedByUser(name) => ErrorType::DeniedByUser(name.clone()),
        ToolError::DeniedByPolicy(name) => ErrorType::DeniedByPolicy(name.clone()),
        ToolError::InternalError(msg) => ErrorType::InternalError(msg.clone()),
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
        ProtoResult::Agent(r) => ToolResult::Agent(AgentResult {
            content: r.content,
            session_id: if r.session_id.is_empty() {
                None
            } else {
                Some(r.session_id)
            },
            workspace: r.workspace.map(proto_to_agent_workspace_info),
        }),
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
    use steer_tools::error::{ToolError, ToolExecutionError};

    let error_type = proto_error
        .error_type
        .ok_or_else(|| ConversionError::MissingField {
            field: "tool_error.error_type".to_string(),
        })?;

    Ok(match error_type {
        ErrorType::UnknownTool(name) => ToolError::UnknownTool(name),
        ErrorType::InvalidParams(e) => ToolError::InvalidParams {
            tool_name: e.tool_name,
            message: e.message,
        },
        ErrorType::Execution(e) => ToolError::Execution(ToolExecutionError::External {
            tool_name: e.tool_name,
            message: e.message,
        }),
        ErrorType::Cancelled(name) => ToolError::Cancelled(name),
        ErrorType::Timeout(name) => ToolError::Timeout(name),
        ErrorType::DeniedByUser(name) => ToolError::DeniedByUser(name),
        ErrorType::DeniedByPolicy(name) => ToolError::DeniedByPolicy(name),
        ErrorType::InternalError(msg) => ToolError::InternalError(msg),
        ErrorType::Io(e) => ToolError::Execution(ToolExecutionError::External {
            tool_name: e.tool_name,
            message: e.message,
        }),
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
                    .map(|user_content| match user_content {
                        UserContent::Text { text } => proto::UserContent {
                            content: Some(proto::user_content::Content::Text(text.clone())),
                        },
                        UserContent::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        } => proto::UserContent {
                            content: Some(proto::user_content::Content::CommandExecution(
                                proto::CommandExecution {
                                    command: command.clone(),
                                    stdout: stdout.clone(),
                                    stderr: stderr.clone(),
                                    exit_code: *exit_code,
                                },
                            )),
                        },
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
                        AssistantContent::ToolCall { tool_call, .. } => proto::AssistantContent {
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

pub(crate) fn tool_approval_policy_to_proto(
    policy: &ToolApprovalPolicy,
) -> proto::ToolApprovalPolicy {
    proto::ToolApprovalPolicy {
        default_behavior: match policy.default_behavior {
            UnapprovedBehavior::Prompt => proto::UnapprovedBehavior::Prompt.into(),
            UnapprovedBehavior::Deny => proto::UnapprovedBehavior::Deny.into(),
            UnapprovedBehavior::Allow => proto::UnapprovedBehavior::Allow.into(),
        },
        preapproved: Some(approval_rules_to_proto(&policy.preapproved)),
    }
}

fn approval_rules_to_proto(rules: &ApprovalRules) -> proto::ApprovalRules {
    proto::ApprovalRules {
        tools: rules.tools.iter().cloned().collect(),
        per_tool: rules
            .per_tool
            .iter()
            .map(|(name, rule)| (name.clone(), tool_rule_to_proto(rule)))
            .collect(),
    }
}

fn tool_rule_to_proto(rule: &ToolRule) -> proto::ToolRule {
    match rule {
        ToolRule::Bash { patterns } => proto::ToolRule {
            rule: Some(proto::tool_rule::Rule::Bash(proto::BashRule {
                patterns: patterns.clone(),
            })),
        },
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
    }
}

pub(crate) fn session_config_to_proto(config: &SessionConfig) -> proto::SessionConfig {
    proto::SessionConfig {
        tool_config: Some(session_tool_config_to_proto(&config.tool_config)),
        metadata: config.metadata.clone(),
        workspace_config: Some(workspace_config_to_proto(&config.workspace)),
        system_prompt: config.system_prompt.clone(),
        default_model: Some(model_to_proto(config.default_model.clone())),
        workspace_id: config.workspace_id.map(|id| id.as_uuid().to_string()),
        workspace_ref: config
            .workspace_ref
            .as_ref()
            .map(|reference| proto::WorkspaceRef {
                environment_id: reference.environment_id.as_uuid().to_string(),
                workspace_id: reference.workspace_id.as_uuid().to_string(),
                repo_id: reference.repo_id.as_uuid().to_string(),
            }),
        parent_session_id: config.parent_session_id.as_ref().map(SessionId::to_string),
        workspace_name: config.workspace_name.clone(),
        repo_ref: config.repo_ref.as_ref().map(repo_ref_to_proto),
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

pub(crate) fn environment_descriptor_to_proto(
    descriptor: &steer_workspace::EnvironmentDescriptor,
) -> proto::EnvironmentDescriptor {
    proto::EnvironmentDescriptor {
        environment_id: descriptor.environment_id.as_uuid().to_string(),
        root_path: descriptor.root.to_string_lossy().to_string(),
    }
}

pub(crate) fn repo_ref_to_proto(reference: &steer_workspace::RepoRef) -> proto::RepoRef {
    proto::RepoRef {
        environment_id: reference.environment_id.as_uuid().to_string(),
        repo_id: reference.repo_id.as_uuid().to_string(),
        root_path: reference.root_path.to_string_lossy().to_string(),
        vcs_kind: reference.vcs_kind.as_ref().map(|kind| match kind {
            steer_workspace::VcsKind::Git => remote_proto::VcsKind::Git as i32,
            steer_workspace::VcsKind::Jj => remote_proto::VcsKind::Jj as i32,
        }),
    }
}

pub(crate) fn repo_info_to_proto(info: &steer_workspace::RepoInfo) -> proto::RepoInfo {
    proto::RepoInfo {
        repo_id: info.repo_id.as_uuid().to_string(),
        environment_id: info.environment_id.as_uuid().to_string(),
        root_path: info.root_path.to_string_lossy().to_string(),
        vcs_kind: info.vcs_kind.as_ref().map(|kind| match kind {
            steer_workspace::VcsKind::Git => remote_proto::VcsKind::Git as i32,
            steer_workspace::VcsKind::Jj => remote_proto::VcsKind::Jj as i32,
        }),
    }
}

pub(crate) fn workspace_info_to_proto(info: &steer_workspace::WorkspaceInfo) -> proto::WorkspaceInfo {
    proto::WorkspaceInfo {
        workspace_id: info.workspace_id.as_uuid().to_string(),
        environment_id: info.environment_id.as_uuid().to_string(),
        parent_workspace_id: info
            .parent_workspace_id
            .map(|id| id.as_uuid().to_string()),
        name: info.name.clone(),
        path: info.path.to_string_lossy().to_string(),
        repo_id: info.repo_id.as_uuid().to_string(),
    }
}

pub(crate) fn workspace_status_to_proto(
    status: &steer_workspace::WorkspaceStatus,
) -> proto::WorkspaceStatus {
    proto::WorkspaceStatus {
        workspace_id: status.workspace_id.as_uuid().to_string(),
        environment_id: status.environment_id.as_uuid().to_string(),
        path: status.path.to_string_lossy().to_string(),
        vcs: status.vcs.as_ref().map(vcs_info_to_proto),
        repo_id: status.repo_id.as_uuid().to_string(),
    }
}

pub(crate) fn proto_to_workspace_info(
    info: proto::WorkspaceInfo,
) -> Result<steer_workspace::WorkspaceInfo, ConversionError> {
    let workspace_id = parse_workspace_id(&info.workspace_id)?;
    let environment_id = parse_environment_id(&info.environment_id)?;
    let repo_id = parse_repo_id(&info.repo_id)?;
    let parent_workspace_id = match info.parent_workspace_id {
        Some(value) if !value.is_empty() => Some(parse_workspace_id(&value)?),
        _ => None,
    };

    Ok(steer_workspace::WorkspaceInfo {
        workspace_id,
        environment_id,
        repo_id,
        parent_workspace_id,
        name: info.name,
        path: PathBuf::from(info.path),
    })
}

pub(crate) fn proto_to_workspace_status(
    status: proto::WorkspaceStatus,
) -> Result<steer_workspace::WorkspaceStatus, ConversionError> {
    let workspace_id = parse_workspace_id(&status.workspace_id)?;
    let environment_id = parse_environment_id(&status.environment_id)?;
    let repo_id = parse_repo_id(&status.repo_id)?;
    let vcs = match status.vcs {
        Some(info) => Some(proto_to_vcs_info(info)?),
        None => None,
    };

    Ok(steer_workspace::WorkspaceStatus {
        workspace_id,
        environment_id,
        repo_id,
        path: PathBuf::from(status.path),
        vcs,
    })
}

pub(crate) fn proto_to_repo_info(
    info: proto::RepoInfo,
) -> Result<steer_workspace::RepoInfo, ConversionError> {
    let repo_id = parse_repo_id(&info.repo_id)?;
    let environment_id = parse_environment_id(&info.environment_id)?;
    let vcs_kind = info
        .vcs_kind
        .and_then(|value| match remote_proto::VcsKind::try_from(value) {
            Ok(remote_proto::VcsKind::Git) => Some(steer_workspace::VcsKind::Git),
            Ok(remote_proto::VcsKind::Jj) => Some(steer_workspace::VcsKind::Jj),
            _ => None,
        });

    Ok(steer_workspace::RepoInfo {
        repo_id,
        environment_id,
        root_path: PathBuf::from(info.root_path),
        vcs_kind,
    })
}

fn parse_environment_id(value: &str) -> Result<steer_workspace::EnvironmentId, ConversionError> {
    if value.is_empty() {
        return Ok(steer_workspace::EnvironmentId::local());
    }
    let uuid = Uuid::parse_str(value).map_err(|e| ConversionError::InvalidData {
        message: format!("Invalid environment_id '{value}': {e}"),
    })?;
    Ok(steer_workspace::EnvironmentId::from_uuid(uuid))
}

fn parse_workspace_id(value: &str) -> Result<steer_workspace::WorkspaceId, ConversionError> {
    if value.is_empty() {
        return Err(ConversionError::InvalidData {
            message: "workspace_id is empty".to_string(),
        });
    }
    let uuid = Uuid::parse_str(value).map_err(|e| ConversionError::InvalidData {
        message: format!("Invalid workspace_id '{value}': {e}"),
    })?;
    Ok(steer_workspace::WorkspaceId::from_uuid(uuid))
}

fn parse_repo_id(value: &str) -> Result<steer_workspace::RepoId, ConversionError> {
    if value.is_empty() {
        return Err(ConversionError::InvalidData {
            message: "repo_id is empty".to_string(),
        });
    }
    let uuid = Uuid::parse_str(value).map_err(|e| ConversionError::InvalidData {
        message: format!("Invalid repo_id '{value}': {e}"),
    })?;
    Ok(steer_workspace::RepoId::from_uuid(uuid))
}

fn proto_to_vcs_info(
    info: remote_proto::VcsInfo,
) -> Result<steer_workspace::VcsInfo, ConversionError> {
    let kind = match remote_proto::VcsKind::try_from(info.kind) {
        Ok(remote_proto::VcsKind::Git) => steer_workspace::VcsKind::Git,
        Ok(remote_proto::VcsKind::Jj) => steer_workspace::VcsKind::Jj,
        _ => {
            return Err(ConversionError::InvalidEnumValue {
                value: info.kind,
                enum_name: "VcsKind".to_string(),
            })
        }
    };

    let status = match info.status {
        Some(remote_proto::vcs_info::Status::GitStatus(status)) => {
            steer_workspace::VcsStatus::Git(proto_to_git_status(status)?)
        }
        Some(remote_proto::vcs_info::Status::JjStatus(status)) => {
            steer_workspace::VcsStatus::Jj(proto_to_jj_status(status)?)
        }
        None => match kind {
            steer_workspace::VcsKind::Git => steer_workspace::VcsStatus::Git(
                steer_workspace::GitStatus::unavailable("Missing git status"),
            ),
            steer_workspace::VcsKind::Jj => steer_workspace::VcsStatus::Jj(
                steer_workspace::JjStatus::unavailable("Missing jj status"),
            ),
        },
    };

    Ok(steer_workspace::VcsInfo {
        kind,
        root: PathBuf::from(info.root),
        status,
    })
}

fn proto_to_git_status(
    status: remote_proto::GitStatus,
) -> Result<steer_workspace::GitStatus, ConversionError> {
    let head = match status.head {
        Some(head) => {
            let kind = remote_proto::GitHeadKind::try_from(head.kind).map_err(|_| {
                ConversionError::InvalidEnumValue {
                    value: head.kind,
                    enum_name: "GitHeadKind".to_string(),
                }
            })?;
            match kind {
                remote_proto::GitHeadKind::Branch => {
                    let branch = head.branch.ok_or_else(|| ConversionError::MissingField {
                        field: "GitHead.branch".to_string(),
                    })?;
                    Some(steer_workspace::GitHead::Branch(branch))
                }
                remote_proto::GitHeadKind::Detached => Some(steer_workspace::GitHead::Detached),
                remote_proto::GitHeadKind::Unborn => Some(steer_workspace::GitHead::Unborn),
                remote_proto::GitHeadKind::Unspecified => None,
            }
        }
        None => None,
    };

    let entries = status
        .entries
        .into_iter()
        .map(|entry| {
            let summary = match remote_proto::GitStatusSummary::try_from(entry.summary) {
                Ok(remote_proto::GitStatusSummary::Added) => {
                    steer_workspace::GitStatusSummary::Added
                }
                Ok(remote_proto::GitStatusSummary::Removed) => {
                    steer_workspace::GitStatusSummary::Removed
                }
                Ok(remote_proto::GitStatusSummary::Modified) => {
                    steer_workspace::GitStatusSummary::Modified
                }
                Ok(remote_proto::GitStatusSummary::TypeChange) => {
                    steer_workspace::GitStatusSummary::TypeChange
                }
                Ok(remote_proto::GitStatusSummary::Renamed) => {
                    steer_workspace::GitStatusSummary::Renamed
                }
                Ok(remote_proto::GitStatusSummary::Copied) => {
                    steer_workspace::GitStatusSummary::Copied
                }
                Ok(remote_proto::GitStatusSummary::IntentToAdd) => {
                    steer_workspace::GitStatusSummary::IntentToAdd
                }
                Ok(remote_proto::GitStatusSummary::Conflict) => {
                    steer_workspace::GitStatusSummary::Conflict
                }
                _ => {
                    return Err(ConversionError::InvalidEnumValue {
                        value: entry.summary,
                        enum_name: "GitStatusSummary".to_string(),
                    })
                }
            };
            Ok(steer_workspace::GitStatusEntry {
                summary,
                path: entry.path,
            })
        })
        .collect::<Result<Vec<_>, ConversionError>>()?;

    let recent_commits = status
        .recent_commits
        .into_iter()
        .map(|commit| steer_workspace::GitCommitSummary {
            id: commit.id,
            summary: commit.summary,
        })
        .collect();

    Ok(steer_workspace::GitStatus {
        head,
        entries,
        recent_commits,
        error: status.error,
    })
}

fn proto_to_jj_status(
    status: remote_proto::JjStatus,
) -> Result<steer_workspace::JjStatus, ConversionError> {
    let changes = status
        .changes
        .into_iter()
        .map(|change| {
            let change_type = match remote_proto::JjChangeType::try_from(change.change_type) {
                Ok(remote_proto::JjChangeType::Added) => steer_workspace::JjChangeType::Added,
                Ok(remote_proto::JjChangeType::Removed) => steer_workspace::JjChangeType::Removed,
                Ok(remote_proto::JjChangeType::Modified) => steer_workspace::JjChangeType::Modified,
                _ => {
                    return Err(ConversionError::InvalidEnumValue {
                        value: change.change_type,
                        enum_name: "JjChangeType".to_string(),
                    })
                }
            };
            Ok(steer_workspace::JjChange {
                change_type,
                path: change.path,
            })
        })
        .collect::<Result<Vec<_>, ConversionError>>()?;

    let working_copy = status.working_copy.map(|summary| steer_workspace::JjCommitSummary {
        change_id: summary.change_id,
        commit_id: summary.commit_id,
        description: summary.description,
    });

    let parents = status
        .parents
        .into_iter()
        .map(|summary| steer_workspace::JjCommitSummary {
            change_id: summary.change_id,
            commit_id: summary.commit_id,
            description: summary.description,
        })
        .collect();

    Ok(steer_workspace::JjStatus {
        changes,
        working_copy,
        parents,
        error: status.error,
    })
}

fn vcs_info_to_proto(info: &steer_workspace::VcsInfo) -> remote_proto::VcsInfo {
    let kind = match info.kind {
        steer_workspace::VcsKind::Git => remote_proto::VcsKind::Git,
        steer_workspace::VcsKind::Jj => remote_proto::VcsKind::Jj,
    };

    let status = match &info.status {
        steer_workspace::VcsStatus::Git(status) => {
            let head = status.head.as_ref().map(|head| {
                let (kind, branch) = match head {
                    steer_workspace::GitHead::Branch(branch) => {
                        (remote_proto::GitHeadKind::Branch, Some(branch.clone()))
                    }
                    steer_workspace::GitHead::Detached => {
                        (remote_proto::GitHeadKind::Detached, None)
                    }
                    steer_workspace::GitHead::Unborn => (remote_proto::GitHeadKind::Unborn, None),
                };
                remote_proto::GitHead {
                    kind: kind as i32,
                    branch,
                }
            });

            let entries = status
                .entries
                .iter()
                .map(|entry| remote_proto::GitStatusEntry {
                    summary: match entry.summary {
                        steer_workspace::GitStatusSummary::Added => {
                            remote_proto::GitStatusSummary::Added as i32
                        }
                        steer_workspace::GitStatusSummary::Removed => {
                            remote_proto::GitStatusSummary::Removed as i32
                        }
                        steer_workspace::GitStatusSummary::Modified => {
                            remote_proto::GitStatusSummary::Modified as i32
                        }
                        steer_workspace::GitStatusSummary::TypeChange => {
                            remote_proto::GitStatusSummary::TypeChange as i32
                        }
                        steer_workspace::GitStatusSummary::Renamed => {
                            remote_proto::GitStatusSummary::Renamed as i32
                        }
                        steer_workspace::GitStatusSummary::Copied => {
                            remote_proto::GitStatusSummary::Copied as i32
                        }
                        steer_workspace::GitStatusSummary::IntentToAdd => {
                            remote_proto::GitStatusSummary::IntentToAdd as i32
                        }
                        steer_workspace::GitStatusSummary::Conflict => {
                            remote_proto::GitStatusSummary::Conflict as i32
                        }
                    },
                    path: entry.path.clone(),
                })
                .collect();

            let recent_commits = status
                .recent_commits
                .iter()
                .map(|commit| remote_proto::GitCommitSummary {
                    id: commit.id.clone(),
                    summary: commit.summary.clone(),
                })
                .collect();

            let git_status = remote_proto::GitStatus {
                head,
                entries,
                recent_commits,
                error: status.error.clone(),
            };
            Some(remote_proto::vcs_info::Status::GitStatus(git_status))
        }
        steer_workspace::VcsStatus::Jj(status) => {
            let changes = status
                .changes
                .iter()
                .map(|change| remote_proto::JjChange {
                    change_type: match change.change_type {
                        steer_workspace::JjChangeType::Added => {
                            remote_proto::JjChangeType::Added as i32
                        }
                        steer_workspace::JjChangeType::Removed => {
                            remote_proto::JjChangeType::Removed as i32
                        }
                        steer_workspace::JjChangeType::Modified => {
                            remote_proto::JjChangeType::Modified as i32
                        }
                    },
                    path: change.path.clone(),
                })
                .collect();

            let working_copy = status.working_copy.as_ref().map(|summary| remote_proto::JjCommitSummary {
                change_id: summary.change_id.clone(),
                commit_id: summary.commit_id.clone(),
                description: summary.description.clone(),
            });

            let parents = status
                .parents
                .iter()
                .map(|summary| remote_proto::JjCommitSummary {
                    change_id: summary.change_id.clone(),
                    commit_id: summary.commit_id.clone(),
                    description: summary.description.clone(),
                })
                .collect();

            let jj_status = remote_proto::JjStatus {
                changes,
                working_copy,
                parents,
                error: status.error.clone(),
            };
            Some(remote_proto::vcs_info::Status::JjStatus(jj_status))
        }
    };

    remote_proto::VcsInfo {
        kind: kind as i32,
        root: info.root.to_string_lossy().to_string(),
        status,
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

pub(crate) fn proto_to_tool_approval_policy(
    proto_policy: Option<proto::ToolApprovalPolicy>,
) -> ToolApprovalPolicy {
    match proto_policy {
        Some(policy) => {
            let default_behavior =
                match proto::UnapprovedBehavior::try_from(policy.default_behavior) {
                    Ok(proto::UnapprovedBehavior::Deny) => UnapprovedBehavior::Deny,
                    Ok(proto::UnapprovedBehavior::Allow) => UnapprovedBehavior::Allow,
                    _ => UnapprovedBehavior::Prompt,
                };
            let preapproved = policy
                .preapproved
                .map(proto_to_approval_rules)
                .unwrap_or_default();
            ToolApprovalPolicy {
                default_behavior,
                preapproved,
            }
        }
        None => ToolApprovalPolicy::default(),
    }
}

fn proto_to_approval_rules(proto_rules: proto::ApprovalRules) -> ApprovalRules {
    ApprovalRules {
        tools: proto_rules.tools.into_iter().collect(),
        per_tool: proto_rules
            .per_tool
            .into_iter()
            .filter_map(|(name, rule)| proto_to_tool_rule(rule).map(|r| (name, r)))
            .collect(),
    }
}

fn proto_to_tool_rule(proto_rule: proto::ToolRule) -> Option<ToolRule> {
    match proto_rule.rule? {
        proto::tool_rule::Rule::Bash(bash) => Some(ToolRule::Bash {
            patterns: bash.patterns,
        }),
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
                    user_content.content.map(|content| match content {
                        user_content::Content::Text(text) => UserContent::Text { text },
                        user_content::Content::CommandExecution(cmd) => {
                            UserContent::CommandExecution {
                                command: cmd.command,
                                stdout: cmd.stdout,
                                stderr: cmd.stderr,
                                exit_code: cmd.exit_code,
                            }
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
                                    thought_signature: None,
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

fn proto_user_message_to_core(
    id: String,
    user_msg: proto::UserMessage,
) -> Result<ConversationMessage, ConversionError> {
    use steer_core::app::conversation::UserContent;
    use steer_proto::agent::v1::user_content;

    let content = user_msg
        .content
        .into_iter()
        .filter_map(|user_content| {
            user_content.content.map(|content| match content {
                user_content::Content::Text(text) => UserContent::Text { text },
                user_content::Content::CommandExecution(cmd) => UserContent::CommandExecution {
                    command: cmd.command,
                    stdout: cmd.stdout,
                    stderr: cmd.stderr,
                    exit_code: cmd.exit_code,
                },
            })
        })
        .collect();

    Ok(ConversationMessage {
        data: MessageData::User { content },
        timestamp: user_msg.timestamp,
        id,
        parent_message_id: user_msg.parent_message_id,
    })
}

fn proto_assistant_message_to_core(
    id: String,
    assistant_msg: proto::AssistantMessage,
) -> Result<ConversationMessage, ConversionError> {
    use steer_core::app::conversation::{AssistantContent, ThoughtContent};
    use steer_proto::agent::v1::{assistant_content, thought_content};

    let content = assistant_msg
        .content
        .into_iter()
        .filter_map(|assistant_content| {
            assistant_content.content.and_then(|content| match content {
                assistant_content::Content::Text(text) => Some(AssistantContent::Text { text }),
                assistant_content::Content::ToolCall(tool_call) => {
                    match proto_tool_call_to_core(&tool_call) {
                        Ok(core_tool_call) => Some(AssistantContent::ToolCall {
                            tool_call: core_tool_call,
                            thought_signature: None,
                        }),
                        Err(_) => None, // Skip invalid tool calls
                    }
                }
                assistant_content::Content::Thought(thought) => {
                    let thought_content = thought.thought_type.as_ref().map(|t| match t {
                        thought_content::ThoughtType::Simple(simple) => ThoughtContent::Simple {
                            text: simple.text.clone(),
                        },
                        thought_content::ThoughtType::Signed(signed) => ThoughtContent::Signed {
                            text: signed.text.clone(),
                            signature: signed.signature.clone(),
                        },
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
        id,
        parent_message_id: assistant_msg.parent_message_id,
    })
}

fn proto_tool_message_to_core(
    id: String,
    tool_msg: proto::ToolMessage,
) -> Result<ConversationMessage, ConversionError> {
    if let Some(proto_result) = tool_msg.result {
        let tool_result = proto_to_steer_tools_result(proto_result)?;
        Ok(ConversationMessage {
            data: MessageData::Tool {
                tool_use_id: tool_msg.tool_use_id,
                result: tool_result,
            },
            timestamp: tool_msg.timestamp,
            id,
            parent_message_id: tool_msg.parent_message_id,
        })
    } else {
        Err(ConversionError::MissingField {
            field: "tool_msg.result".to_string(),
        })
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

fn compaction_record_from_proto(
    record: proto::CompactionRecord,
) -> Result<steer_core::app::domain::types::CompactionRecord, ConversionError> {
    use chrono::{DateTime, Utc};
    use steer_core::app::domain::types::{CompactionId, MessageId};

    let id = uuid::Uuid::parse_str(&record.id).map_err(|_| ConversionError::InvalidData {
        message: format!("Invalid compaction record id: {}", record.id),
    })?;

    let created_at = record
        .created_at
        .and_then(|ts| DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32))
        .unwrap_or_else(Utc::now);

    Ok(steer_core::app::domain::types::CompactionRecord {
        id: CompactionId::from(id),
        summary_message_id: MessageId::from_string(record.summary_message_id),
        compacted_head_message_id: MessageId::from_string(record.compacted_head_message_id),
        previous_active_message_id: record
            .previous_active_message_id
            .map(MessageId::from_string),
        model: record.model,
        created_at,
    })
}

fn compact_result_to_proto(
    result: &steer_core::app::domain::event::CompactResult,
) -> proto::CompactResult {
    let result = match result {
        steer_core::app::domain::event::CompactResult::Success(summary) => {
            proto::compact_result::Result::Success(proto::CompactSuccess {
                summary: summary.clone(),
            })
        }
        steer_core::app::domain::event::CompactResult::Cancelled => {
            proto::compact_result::Result::Cancelled(proto::CompactCancelled {})
        }
        steer_core::app::domain::event::CompactResult::InsufficientMessages => {
            proto::compact_result::Result::InsufficientMessages(
                proto::CompactInsufficientMessages {},
            )
        }
    };

    proto::CompactResult {
        result: Some(result),
    }
}

fn compact_result_from_proto(
    result: proto::CompactResult,
) -> Result<steer_core::app::domain::event::CompactResult, ConversionError> {
    let Some(result) = result.result else {
        return Err(ConversionError::MissingField {
            field: "compact_result.result".to_string(),
        });
    };

    Ok(match result {
        proto::compact_result::Result::Success(success) => {
            steer_core::app::domain::event::CompactResult::Success(success.summary)
        }
        proto::compact_result::Result::Cancelled(_) => {
            steer_core::app::domain::event::CompactResult::Cancelled
        }
        proto::compact_result::Result::InsufficientMessages(_) => {
            steer_core::app::domain::event::CompactResult::InsufficientMessages
        }
    })
}

/// Convert domain SessionEvent to protobuf SessionEvent
///
/// This is used by the new RuntimeService architecture to convert events
/// from the pure reducer-based domain model to the gRPC protocol.
pub(crate) fn session_event_to_proto(
    session_event: SessionEvent,
    sequence_num: u64,
) -> Result<proto::SessionEvent, ConversionError> {
    let timestamp = Some(prost_types::Timestamp::from(std::time::SystemTime::now()));

    let event = match session_event {
        SessionEvent::SessionCreated { .. } => None,
        SessionEvent::SessionConfigUpdated { .. } => None,
        SessionEvent::AssistantMessageAdded { message, model } => {
            let proto_message = message_to_proto(message)?;
            let id = proto_message.id;
            let message_variant = proto_message.message;
            let assistant_message = match message_variant {
                Some(proto::message::Message::Assistant(assistant_message)) => assistant_message,
                Some(proto::message::Message::User(_)) => {
                    return Err(ConversionError::InvalidVariant {
                        expected: "assistant".to_string(),
                        actual: "user".to_string(),
                    });
                }
                Some(proto::message::Message::Tool(_)) => {
                    return Err(ConversionError::InvalidVariant {
                        expected: "assistant".to_string(),
                        actual: "tool".to_string(),
                    });
                }
                None => {
                    return Err(ConversionError::MissingField {
                        field: "message".to_string(),
                    });
                }
            };

            Some(proto::session_event::Event::AssistantMessageAdded(
                proto::AssistantMessageAddedEvent {
                    id,
                    message: Some(assistant_message),
                    model: Some(model_to_proto(model)),
                },
            ))
        }
        SessionEvent::UserMessageAdded { message } => {
            let proto_message = message_to_proto(message)?;
            let id = proto_message.id;
            let message_variant = proto_message.message;
            let user_message = match message_variant {
                Some(proto::message::Message::User(user_message)) => user_message,
                Some(proto::message::Message::Assistant(_)) => {
                    return Err(ConversionError::InvalidVariant {
                        expected: "user".to_string(),
                        actual: "assistant".to_string(),
                    });
                }
                Some(proto::message::Message::Tool(_)) => {
                    return Err(ConversionError::InvalidVariant {
                        expected: "user".to_string(),
                        actual: "tool".to_string(),
                    });
                }
                None => {
                    return Err(ConversionError::MissingField {
                        field: "message".to_string(),
                    });
                }
            };

            Some(proto::session_event::Event::UserMessageAdded(
                proto::UserMessageAddedEvent {
                    id,
                    message: Some(user_message),
                },
            ))
        }
        SessionEvent::ToolMessageAdded { message } => {
            let proto_message = message_to_proto(message)?;
            let id = proto_message.id;
            let message_variant = proto_message.message;
            let tool_message = match message_variant {
                Some(proto::message::Message::Tool(tool_message)) => tool_message,
                Some(proto::message::Message::Assistant(_)) => {
                    return Err(ConversionError::InvalidVariant {
                        expected: "tool".to_string(),
                        actual: "assistant".to_string(),
                    });
                }
                Some(proto::message::Message::User(_)) => {
                    return Err(ConversionError::InvalidVariant {
                        expected: "tool".to_string(),
                        actual: "user".to_string(),
                    });
                }
                None => {
                    return Err(ConversionError::MissingField {
                        field: "message".to_string(),
                    });
                }
            };

            Some(proto::session_event::Event::ToolMessageAdded(
                proto::ToolMessageAddedEvent {
                    id,
                    message: Some(tool_message),
                },
            ))
        }
        SessionEvent::MessageUpdated { message } => {
            let proto_message = message_to_proto(message)?;
            Some(proto::session_event::Event::MessageUpdated(
                proto::MessageUpdatedEvent {
                    message: Some(proto_message),
                },
            ))
        }
        SessionEvent::ToolCallStarted {
            id,
            name,
            parameters,
            model,
        } => Some(proto::session_event::Event::ToolCallStarted(
            proto::ToolCallStartedEvent {
                name,
                id: id.to_string(),
                model: Some(model_to_proto(model)),
                parameters_json: serde_json::to_string(&parameters).unwrap_or_default(),
            },
        )),
        SessionEvent::ToolCallCompleted {
            id,
            name,
            result,
            model,
        } => {
            let proto_result = steer_tools_result_to_proto(&result)
                .map_err(|e| ConversionError::ToolResultConversion(e.to_string()))?;
            Some(proto::session_event::Event::ToolCallCompleted(
                proto::ToolCallCompletedEvent {
                    name,
                    result: Some(proto_result),
                    id: id.to_string(),
                    model: Some(model_to_proto(model)),
                },
            ))
        }
        SessionEvent::ToolCallFailed {
            id,
            name,
            error,
            model,
        } => Some(proto::session_event::Event::ToolCallFailed(
            proto::ToolCallFailedEvent {
                name,
                error,
                id: id.to_string(),
                model: Some(model_to_proto(model)),
            },
        )),
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
        SessionEvent::CompactResult { result } => Some(proto::session_event::Event::CompactResult(
            proto::CompactResultEvent {
                result: Some(compact_result_to_proto(&result)),
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
        SessionEvent::McpServerStateChanged { server_name, state } => Some(
            proto::session_event::Event::McpServerStateChanged(proto::McpServerStateChangedEvent {
                server_name,
                state: Some(mcp_server_state_to_proto(&state)),
            }),
        ),
    };

    Ok(proto::SessionEvent {
        sequence_num,
        timestamp,
        event,
    })
}

pub(crate) fn stream_delta_to_proto(
    delta: StreamDelta,
    sequence_num: u64,
    delta_sequence: u64,
) -> Result<proto::SessionEvent, ConversionError> {
    let timestamp = Some(prost_types::Timestamp::from(std::time::SystemTime::now()));

    let (op_id, message_id, delta_type) = match delta {
        StreamDelta::TextChunk {
            op_id,
            message_id,
            delta,
        } => (
            op_id,
            message_id,
            proto::stream_delta_event::DeltaType::Text(proto::TextDelta { content: delta }),
        ),
        StreamDelta::ThinkingChunk {
            op_id,
            message_id,
            delta,
        } => (
            op_id,
            message_id,
            proto::stream_delta_event::DeltaType::Thinking(proto::ThinkingDelta { content: delta }),
        ),
        StreamDelta::ToolCallChunk {
            op_id,
            message_id,
            tool_call_id,
            delta,
        } => {
            let delta = match delta {
                CoreToolCallDelta::Name(name) => proto::tool_call_delta::Delta::Name(name),
                CoreToolCallDelta::ArgumentChunk(chunk) => {
                    proto::tool_call_delta::Delta::ArgumentChunk(chunk)
                }
            };

            let tool_call = proto::ToolCallDelta {
                tool_call_id: tool_call_id.to_string(),
                delta: Some(delta),
            };

            (
                op_id,
                message_id,
                proto::stream_delta_event::DeltaType::ToolCall(tool_call),
            )
        }
    };

    Ok(proto::SessionEvent {
        sequence_num,
        timestamp,
        event: Some(proto::session_event::Event::StreamDelta(
            proto::StreamDeltaEvent {
                op_id: op_id.to_string(),
                message_id: message_id.to_string(),
                delta_sequence,
                delta_type: Some(delta_type),
            },
        )),
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

pub(crate) fn mcp_server_state_to_proto(
    state: &steer_core::app::domain::action::McpServerState,
) -> proto::McpConnectionState {
    use steer_core::app::domain::action::McpServerState;

    match state {
        McpServerState::Connecting => proto::McpConnectionState {
            state: Some(proto::mcp_connection_state::State::Connecting(
                proto::McpConnecting {},
            )),
        },
        McpServerState::Connected { tools } => {
            let tool_names = tools.iter().map(|t| t.name.clone()).collect();
            proto::McpConnectionState {
                state: Some(proto::mcp_connection_state::State::Connected(
                    proto::McpConnected { tool_names },
                )),
            }
        }
        McpServerState::Disconnected { error } => proto::McpConnectionState {
            state: Some(proto::mcp_connection_state::State::Disconnected(
                proto::McpDisconnected {
                    reason: error.clone(),
                },
            )),
        },
        McpServerState::Failed { error } => proto::McpConnectionState {
            state: Some(proto::mcp_connection_state::State::Failed(
                proto::McpFailed {
                    error: error.clone(),
                },
            )),
        },
    }
}

pub(crate) fn proto_to_mcp_server_state(
    proto: proto::McpConnectionState,
) -> Result<steer_core::app::domain::action::McpServerState, ConversionError> {
    use steer_core::app::domain::action::McpServerState;

    match proto.state {
        Some(proto::mcp_connection_state::State::Connecting(_)) => Ok(McpServerState::Connecting),
        Some(proto::mcp_connection_state::State::Connected(connected)) => {
            let tools = connected
                .tool_names
                .into_iter()
                .map(|name| steer_tools::ToolSchema {
                    name: name.clone(),
                    display_name: name,
                    description: String::new(),
                    input_schema: steer_tools::InputSchema {
                        properties: Default::default(),
                        required: Vec::new(),
                        schema_type: "object".to_string(),
                    },
                })
                .collect();
            Ok(McpServerState::Connected { tools })
        }
        Some(proto::mcp_connection_state::State::Failed(failed)) => Ok(McpServerState::Failed {
            error: failed.error,
        }),
        Some(proto::mcp_connection_state::State::Disconnected(disconnected)) => {
            Ok(McpServerState::Disconnected {
                error: disconnected.reason,
            })
        }
        None => Err(ConversionError::MissingField {
            field: "state.state".to_string(),
        }),
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
        Some(proto::mcp_connection_state::State::Disconnected(disconnected)) => {
            McpConnectionState::Disconnected {
                reason: disconnected.reason,
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
            message: format!("Invalid op_id: {s}"),
        })
}

fn parse_request_id(s: &str) -> Result<crate::client_api::RequestId, ConversionError> {
    uuid::Uuid::parse_str(s)
        .map(crate::client_api::RequestId::from)
        .map_err(|_| ConversionError::InvalidData {
            message: format!("Invalid request_id: {s}"),
        })
}

pub(crate) fn proto_to_client_event(
    server_event: proto::SessionEvent,
) -> Result<Option<crate::client_api::ClientEvent>, ConversionError> {
    use crate::client_api::{ClientEvent, MessageId, ToolCallDelta, ToolCallId};
    use steer_tools::ToolCall;

    let event = match server_event.event {
        None => return Ok(None),
        Some(e) => e,
    };

    let client_event = match event {
        proto::session_event::Event::AssistantMessageAdded(e) => {
            let assistant_message = e.message.ok_or_else(|| ConversionError::MissingField {
                field: "assistant_message_added_event.message".to_string(),
            })?;
            let message = proto_assistant_message_to_core(e.id, assistant_message)?;
            let model = e
                .model
                .ok_or_else(|| ConversionError::MissingField {
                    field: "model".to_string(),
                })
                .and_then(|spec| proto_to_model(&spec))?;
            ClientEvent::AssistantMessageAdded { message, model }
        }
        proto::session_event::Event::UserMessageAdded(e) => {
            let user_message = e.message.ok_or_else(|| ConversionError::MissingField {
                field: "user_message_added_event.message".to_string(),
            })?;
            let message = proto_user_message_to_core(e.id, user_message)?;
            ClientEvent::UserMessageAdded { message }
        }
        proto::session_event::Event::ToolMessageAdded(e) => {
            let tool_message = e.message.ok_or_else(|| ConversionError::MissingField {
                field: "tool_message_added_event.message".to_string(),
            })?;
            let message = proto_tool_message_to_core(e.id, tool_message)?;
            ClientEvent::ToolMessageAdded { message }
        }
        proto::session_event::Event::MessageUpdated(e) => {
            let proto_message = e.message.ok_or_else(|| ConversionError::MissingField {
                field: "message_updated_event.message".to_string(),
            })?;
            let message = proto_to_message(proto_message)?;
            ClientEvent::MessageUpdated { message }
        }
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
        proto::session_event::Event::StreamDelta(e) => {
            let op_id = parse_op_id(&e.op_id)?;
            let message_id = MessageId::from_string(e.message_id);
            let delta = e.delta_type.ok_or_else(|| ConversionError::MissingField {
                field: "stream_delta.delta_type".to_string(),
            })?;

            match delta {
                proto::stream_delta_event::DeltaType::Text(text) => ClientEvent::MessageDelta {
                    id: message_id.clone(),
                    delta: text.content,
                },
                proto::stream_delta_event::DeltaType::Thinking(thinking) => {
                    ClientEvent::ThinkingDelta {
                        op_id,
                        message_id: message_id.clone(),
                        delta: thinking.content,
                    }
                }
                proto::stream_delta_event::DeltaType::ToolCall(tool_call) => {
                    let tool_call_id = ToolCallId::from_string(tool_call.tool_call_id);
                    let delta = tool_call
                        .delta
                        .ok_or_else(|| ConversionError::MissingField {
                            field: "stream_delta.tool_call.delta".to_string(),
                        })?;
                    let delta = match delta {
                        proto::tool_call_delta::Delta::Name(name) => ToolCallDelta::Name(name),
                        proto::tool_call_delta::Delta::ArgumentChunk(chunk) => {
                            ToolCallDelta::ArgumentChunk(chunk)
                        }
                    };

                    ClientEvent::ToolCallDelta {
                        op_id,
                        message_id: message_id.clone(),
                        tool_call_id,
                        delta,
                    }
                }
            }
        }
        proto::session_event::Event::CompactResult(e) => {
            let result = e.result.ok_or_else(|| ConversionError::MissingField {
                field: "compact_result.result".to_string(),
            })?;
            let result = compact_result_from_proto(result)?;
            ClientEvent::CompactResult { result }
        }
        proto::session_event::Event::ConversationCompacted(e) => {
            let record = e.record.ok_or_else(|| ConversionError::MissingField {
                field: "conversation_compacted.record".to_string(),
            })?;
            let record = compaction_record_from_proto(record)?;
            ClientEvent::ConversationCompacted { record }
        }
        proto::session_event::Event::Error(e) => ClientEvent::Error { message: e.message },
        proto::session_event::Event::WorkspaceChanged(_) => ClientEvent::WorkspaceChanged,
        proto::session_event::Event::McpServerStateChanged(e) => {
            let state = e.state.ok_or_else(|| ConversionError::MissingField {
                field: "mcp_server_state_changed.state".to_string(),
            })?;
            let mcp_state = proto_to_mcp_server_state(state)?;
            ClientEvent::McpServerStateChanged {
                server_name: e.server_name,
                state: mcp_state,
            }
        }
    };

    Ok(Some(client_event))
}

pub(crate) fn auth_progress_to_proto(
    progress: steer_core::auth::AuthProgress,
) -> proto::AuthProgress {
    use proto::auth_progress::State;

    let state = match progress {
        steer_core::auth::AuthProgress::NeedInput(prompt) => {
            State::NeedInput(proto::AuthNeedInput { prompt })
        }
        steer_core::auth::AuthProgress::InProgress(message) => {
            State::InProgress(proto::AuthInProgress { message })
        }
        steer_core::auth::AuthProgress::Complete => State::Complete(proto::AuthComplete {}),
        steer_core::auth::AuthProgress::Error(message) => {
            State::Error(proto::AuthError { message })
        }
        steer_core::auth::AuthProgress::OAuthStarted { auth_url } => {
            State::OauthStarted(proto::AuthOAuthStarted { auth_url })
        }
    };

    proto::AuthProgress { state: Some(state) }
}

pub(crate) fn auth_source_to_proto(source: steer_core::auth::AuthSource) -> proto::AuthSource {
    use proto::auth_source::Source;

    match source {
        steer_core::auth::AuthSource::ApiKey { origin } => {
            let origin = match origin {
                steer_core::auth::ApiKeyOrigin::Env => proto::ApiKeyOrigin::Env as i32,
                steer_core::auth::ApiKeyOrigin::Stored => proto::ApiKeyOrigin::Stored as i32,
            };
            proto::AuthSource {
                source: Some(Source::ApiKey(proto::AuthSourceApiKey { origin })),
            }
        }
        steer_core::auth::AuthSource::Plugin { method } => {
            let method = match method {
                steer_core::auth::AuthMethod::OAuth => proto::AuthMethod::Oauth as i32,
                steer_core::auth::AuthMethod::ApiKey => proto::AuthMethod::ApiKey as i32,
            };
            proto::AuthSource {
                source: Some(Source::Plugin(proto::AuthSourcePlugin { method })),
            }
        }
        steer_core::auth::AuthSource::None => proto::AuthSource {
            source: Some(Source::None(proto::AuthSourceNone {})),
        },
    }
}
