use super::types::{
    CompactResult, CompactTrigger, CompactionRecord, ContextWindowUsage, McpServerState, Message,
    MessageId, ModelId, OpId, QueuedWorkItem, RequestId, SessionConfig, TokenUsage, ToolCall,
    ToolCallDelta, ToolCallId, ToolResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageUpdateKind {
    Unspecified,
    Partial,
    Final,
}

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
    LlmUsageUpdated {
        op_id: OpId,
        model: ModelId,
        usage: TokenUsage,
        context_window: Option<ContextWindowUsage>,
        kind: UsageUpdateKind,
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
        trigger: CompactTrigger,
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
        popped_queued_item: Option<QueuedWorkItem>,
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
