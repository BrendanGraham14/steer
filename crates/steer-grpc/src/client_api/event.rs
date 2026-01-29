use super::types::{
    CompactResult, CompactionRecord, McpServerState, Message, MessageId, ModelId, OpId,
    QueuedWorkItem, RequestId, SessionConfig, ToolCall, ToolCallDelta, ToolCallId, ToolResult,
};

#[derive(Debug, Clone)]
pub enum ClientEvent {
    AssistantMessageAdded {
        message: Message,
        model: ModelId,
    },
    UserMessageAdded {
        message: Message,
    },
    ToolMessageAdded {
        message: Message,
    },
    MessageUpdated {
        message: Message,
    },
    MessageDelta {
        id: MessageId,
        delta: String,
    },
    ThinkingDelta {
        op_id: OpId,
        message_id: MessageId,
        delta: String,
    },
    ToolCallDelta {
        op_id: OpId,
        message_id: MessageId,
        tool_call_id: ToolCallId,
        delta: ToolCallDelta,
    },

    CompactResult {
        result: CompactResult,
    },

    ConversationCompacted {
        record: CompactionRecord,
    },

    ToolStarted {
        id: ToolCallId,
        name: String,
        parameters: serde_json::Value,
    },
    ToolCompleted {
        id: ToolCallId,
        name: String,
        result: ToolResult,
    },
    ToolFailed {
        id: ToolCallId,
        name: String,
        error: String,
    },

    ApprovalRequested {
        request_id: RequestId,
        tool_call: ToolCall,
    },

    ProcessingStarted {
        op_id: OpId,
    },
    ProcessingCompleted {
        op_id: OpId,
    },
    OperationCancelled {
        op_id: OpId,
        pending_tool_calls: usize,
    },
    WorkspaceChanged,
    WorkspaceFiles {
        files: Vec<String>,
    },
    Error {
        message: String,
    },
    McpServerStateChanged {
        server_name: String,
        state: McpServerState,
    },
    SessionConfigUpdated {
        config: Box<SessionConfig>,
        primary_agent_id: String,
    },

    QueueUpdated {
        head: Option<QueuedWorkItem>,
        count: usize,
    },
}
