use crate::app::conversation::Message;
use crate::app::domain::action::{ApprovalDecision, ApprovalMemory, McpServerState};
use crate::app::domain::types::{
    CompactionRecord, MessageId, OpId, RequestId, SessionId, ToolCallId,
};
use crate::config::model::ModelId;
use crate::session::state::SessionConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use steer_tools::ToolCall;
use steer_tools::result::ToolResult;

pub use crate::app::domain::state::OperationKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionEvent {
    /// Session was created. For sub-agent sessions, `parent_session_id` links
    /// to the parent session for auditability.
    SessionCreated {
        config: Box<SessionConfig>,
        metadata: HashMap<String, String>,
        /// If this is a sub-agent session, the parent session ID
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_session_id: Option<SessionId>,
    },

    SessionConfigUpdated {
        config: Box<SessionConfig>,
        primary_agent_id: String,
    },

    /// Assistant-authored message; includes the model that produced it.
    AssistantMessageAdded {
        message: Message,
        model: ModelId,
    },

    /// User-authored message; no model attribution.
    UserMessageAdded {
        message: Message,
    },

    /// Tool result message; no model attribution.
    ToolMessageAdded {
        message: Message,
    },

    MessageUpdated {
        message: Message,
    },

    ToolCallStarted {
        id: ToolCallId,
        name: String,
        parameters: serde_json::Value,
        model: ModelId,
    },

    ToolCallCompleted {
        id: ToolCallId,
        name: String,
        result: ToolResult,
        model: ModelId,
    },

    ToolCallFailed {
        id: ToolCallId,
        name: String,
        error: String,
        model: ModelId,
    },

    ApprovalRequested {
        request_id: RequestId,
        tool_call: ToolCall,
    },

    ApprovalDecided {
        request_id: RequestId,
        decision: ApprovalDecision,
        remember: Option<ApprovalMemory>,
    },

    OperationStarted {
        op_id: OpId,
        kind: OperationKind,
    },

    OperationCompleted {
        op_id: OpId,
    },

    OperationCancelled {
        op_id: OpId,
        info: CancellationInfo,
    },

    CompactResult {
        result: CompactResult,
    },

    ConversationCompacted {
        record: CompactionRecord,
    },

    WorkspaceChanged,

    QueueUpdated {
        queue: Vec<QueuedWorkItemSnapshot>,
    },

    Error {
        message: String,
    },

    McpServerStateChanged {
        server_name: String,
        state: McpServerState,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "result_type", rename_all = "snake_case")]
pub enum CompactResult {
    Success(String),
    Cancelled,
    InsufficientMessages,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancellationInfo {
    pub pending_tool_calls: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedWorkItemSnapshot {
    pub kind: Option<QueuedWorkKind>,
    pub content: String,
    pub queued_at: u64,
    pub model: Option<ModelId>,
    pub op_id: OpId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueuedWorkKind {
    UserMessage,
    DirectBash,
}

impl SessionEvent {
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            SessionEvent::Error { .. } | SessionEvent::ToolCallFailed { .. }
        )
    }

    pub fn operation_id(&self) -> Option<OpId> {
        match self {
            SessionEvent::OperationStarted { op_id, .. }
            | SessionEvent::OperationCompleted { op_id }
            | SessionEvent::OperationCancelled { op_id, .. } => Some(*op_id),
            _ => None,
        }
    }

    pub fn tool_call_id(&self) -> Option<&ToolCallId> {
        match self {
            SessionEvent::ToolCallStarted { id, .. }
            | SessionEvent::ToolCallCompleted { id, .. }
            | SessionEvent::ToolCallFailed { id, .. } => Some(id),
            _ => None,
        }
    }
}
