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

#[derive(Debug, Clone, PartialEq)]
pub enum CompactResult {
    Success(String),
    Cancelled,
    InsufficientMessages,
}

impl Serialize for CompactResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        match self {
            CompactResult::Success(summary) => {
                let mut state = serializer.serialize_struct("CompactResult", 2)?;
                state.serialize_field("result_type", "success")?;
                state.serialize_field("summary", summary)?;
                state.end()
            }
            CompactResult::Cancelled => {
                let mut state = serializer.serialize_struct("CompactResult", 1)?;
                state.serialize_field("result_type", "cancelled")?;
                state.end()
            }
            CompactResult::InsufficientMessages => {
                let mut state = serializer.serialize_struct("CompactResult", 1)?;
                state.serialize_field("result_type", "insufficient_messages")?;
                state.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for CompactResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct CompactResultPayload {
            result_type: String,
            #[serde(default)]
            summary: Option<String>,
            #[serde(default)]
            success: Option<String>,
        }

        let payload = CompactResultPayload::deserialize(deserializer)?;
        match payload.result_type.as_str() {
            "success" => {
                let summary = payload
                    .summary
                    .or(payload.success)
                    .ok_or_else(|| serde::de::Error::missing_field("summary"))?;
                Ok(CompactResult::Success(summary))
            }
            "cancelled" => Ok(CompactResult::Cancelled),
            "insufficient_messages" => Ok(CompactResult::InsufficientMessages),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["success", "cancelled", "insufficient_messages"],
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancellationInfo {
    pub pending_tool_calls: usize,
    #[serde(default)]
    pub popped_queued_item: Option<QueuedWorkItemSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedWorkItemSnapshot {
    pub kind: Option<QueuedWorkKind>,
    pub content: String,
    pub queued_at: u64,
    pub model: Option<ModelId>,
    pub op_id: OpId,
    pub message_id: MessageId,
    #[serde(default)]
    pub attachment_count: u32,
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
