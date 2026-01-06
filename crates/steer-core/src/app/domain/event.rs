use crate::app::conversation::Message;
use crate::app::domain::action::{ApprovalDecision, ApprovalMemory};
use crate::app::domain::types::{MessageId, OpId, RequestId, SessionId, ToolCallId};
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
        config: SessionConfig,
        metadata: HashMap<String, String>,
        /// If this is a sub-agent session, the parent session ID
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_session_id: Option<SessionId>,
    },

    MessageAdded {
        message: Message,
        model: ModelId,
    },

    MessageUpdated {
        id: MessageId,
        content: String,
    },

    ToolCallStarted {
        id: ToolCallId,
        name: String,
        parameters: serde_json::Value,
    },

    ToolCallCompleted {
        id: ToolCallId,
        name: String,
        result: ToolResult,
    },

    ToolCallFailed {
        id: ToolCallId,
        name: String,
        error: String,
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

    ModelChanged {
        model: ModelId,
    },

    WorkspaceChanged,

    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancellationInfo {
    pub pending_tool_calls: usize,
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
