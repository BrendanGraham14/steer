//! Stable client-facing types. Import from here, not from `steer_core` or `steer_tools`.

use std::collections::HashMap;

pub use steer_core::app::conversation::{
    AssistantContent, Message, MessageData, ThoughtContent, UserContent,
};

pub use steer_core::app::domain::types::{
    CompactionRecord, MessageId, OpId, RequestId, ToolCallId,
};
pub use steer_core::config::model::ModelId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuedWorkKind {
    UserMessage,
    DirectBash,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedWorkItem {
    pub kind: QueuedWorkKind,
    pub content: String,
    pub model: Option<ModelId>,
    pub queued_at: u64,
    pub op_id: OpId,
    pub message_id: MessageId,
}

pub use steer_core::app::domain::delta::ToolCallDelta;
pub use steer_core::app::domain::event::CompactResult;

pub use steer_core::agents::default_agent_spec_id;

pub use steer_core::config::model::builtin;

pub use steer_core::config::provider;

pub use steer_tools::result::{
    BashResult, EditResult, ExternalResult, FileContentResult, FileListResult, GlobResult,
    GrepResult, ReplaceResult, SearchMatch, SearchResult, TodoListResult, ToolResult,
};
pub use steer_tools::{ToolCall, ToolError};

pub use steer_core::session::state::{
    SessionConfig, SessionPolicyOverrides, SessionToolConfig, WorkspaceConfig,
};

pub use steer_core::session::McpServerInfo;
pub use steer_core::session::state::McpConnectionState;

pub use steer_core::app::domain::action::McpServerState;

pub use steer_core::tools::McpTransport;

pub use steer_core::preferences::{EditingMode, Preferences};

pub use steer_core::config::provider::ProviderId;

pub use steer_workspace::{LlmStatus, WorkspaceStatus};

#[derive(Debug, Clone)]
pub struct CreateSessionParams {
    pub workspace: WorkspaceConfig,
    pub tool_config: SessionToolConfig,
    pub metadata: HashMap<String, String>,
    pub default_model: ModelId,
    pub primary_agent_id: Option<String>,
    pub policy_overrides: SessionPolicyOverrides,
}

impl From<SessionConfig> for CreateSessionParams {
    fn from(config: SessionConfig) -> Self {
        Self {
            workspace: config.workspace,
            tool_config: config.tool_config,
            metadata: config.metadata,
            default_model: config.default_model,
            primary_agent_id: config.primary_agent_id,
            policy_overrides: config.policy_overrides,
        }
    }
}
