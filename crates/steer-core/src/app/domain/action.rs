use crate::app::domain::types::{
    CompactionId, MessageId, NonEmptyString, OpId, RequestId, SessionId, ToolCallId,
};
use serde::{Deserialize, Serialize};
use steer_tools::result::ToolResult;
use steer_tools::{ToolCall, ToolError, ToolSchema};

use super::event::SessionEvent;

#[derive(Debug, Clone)]
pub enum Action {
    UserInput {
        session_id: SessionId,
        text: NonEmptyString,
        op_id: OpId,
        message_id: MessageId,
        timestamp: u64,
    },

    UserEditedMessage {
        session_id: SessionId,
        message_id: MessageId,
        new_content: String,
        op_id: OpId,
        new_message_id: MessageId,
        timestamp: u64,
    },

    ToolApprovalRequested {
        session_id: SessionId,
        request_id: RequestId,
        tool_call: ToolCall,
    },

    ToolApprovalDecided {
        session_id: SessionId,
        request_id: RequestId,
        decision: ApprovalDecision,
        remember: Option<ApprovalMemory>,
    },

    ToolExecutionStarted {
        session_id: SessionId,
        tool_call_id: ToolCallId,
        tool_name: String,
        tool_parameters: serde_json::Value,
    },

    ToolResult {
        session_id: SessionId,
        tool_call_id: ToolCallId,
        tool_name: String,
        result: Result<ToolResult, ToolError>,
    },

    ToolSchemasAvailable {
        session_id: SessionId,
        tools: Vec<ToolSchema>,
    },

    ToolSchemasUpdated {
        session_id: SessionId,
        schemas: Vec<ToolSchema>,
        source: SchemaSource,
    },

    McpServerStateChanged {
        session_id: SessionId,
        server_name: String,
        state: McpServerState,
    },

    ModelResponseComplete {
        session_id: SessionId,
        op_id: OpId,
        message_id: MessageId,
        content: Vec<crate::app::conversation::AssistantContent>,
        timestamp: u64,
    },

    ModelResponseError {
        session_id: SessionId,
        op_id: OpId,
        error: String,
    },

    Cancel {
        session_id: SessionId,
        op_id: Option<OpId>,
    },

    DirectBashCommand {
        session_id: SessionId,
        op_id: OpId,
        command: String,
    },

    RequestCompaction {
        session_id: SessionId,
        op_id: OpId,
    },

    Shutdown,

    Hydrate {
        session_id: SessionId,
        events: Vec<SessionEvent>,
        starting_sequence: u64,
    },

    WorkspaceFilesListed {
        session_id: SessionId,
        files: Vec<String>,
    },

    ModelResolved {
        session_id: SessionId,
        model: crate::config::model::ModelId,
    },

    CompactionComplete {
        session_id: SessionId,
        op_id: OpId,
        compaction_id: CompactionId,
        summary_message_id: MessageId,
        summary: String,
        compacted_head_message_id: MessageId,
        previous_active_message_id: Option<MessageId>,
        model: String,
        timestamp: u64,
    },

    CompactionFailed {
        session_id: SessionId,
        op_id: OpId,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApprovalMemory {
    Tool(String),
    BashPattern(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchemaSource {
    Workspace,
    Mcp { server_name: String },
    Backend { backend_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpServerState {
    Connecting,
    Connected { tools: Vec<ToolSchema> },
    Disconnected { error: Option<String> },
    Failed { error: String },
}

impl Action {
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Action::UserInput { session_id, .. }
            | Action::UserEditedMessage { session_id, .. }
            | Action::ToolApprovalRequested { session_id, .. }
            | Action::ToolApprovalDecided { session_id, .. }
            | Action::ToolExecutionStarted { session_id, .. }
            | Action::ToolResult { session_id, .. }
            | Action::ToolSchemasAvailable { session_id, .. }
            | Action::ToolSchemasUpdated { session_id, .. }
            | Action::McpServerStateChanged { session_id, .. }
            | Action::ModelResponseComplete { session_id, .. }
            | Action::ModelResponseError { session_id, .. }
            | Action::Cancel { session_id, .. }
            | Action::DirectBashCommand { session_id, .. }
            | Action::RequestCompaction { session_id, .. }
            | Action::Hydrate { session_id, .. }
            | Action::WorkspaceFilesListed { session_id, .. }
            | Action::ModelResolved { session_id, .. }
            | Action::CompactionComplete { session_id, .. }
            | Action::CompactionFailed { session_id, .. } => Some(*session_id),
            Action::Shutdown => None,
        }
    }

    pub fn op_id(&self) -> Option<OpId> {
        match self {
            Action::UserInput { op_id, .. }
            | Action::UserEditedMessage { op_id, .. }
            | Action::DirectBashCommand { op_id, .. }
            | Action::RequestCompaction { op_id, .. }
            | Action::ModelResponseComplete { op_id, .. }
            | Action::ModelResponseError { op_id, .. }
            | Action::CompactionComplete { op_id, .. }
            | Action::CompactionFailed { op_id, .. } => Some(*op_id),
            Action::Cancel { op_id, .. } => *op_id,
            _ => None,
        }
    }
}
