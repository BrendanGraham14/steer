use super::types::*;

#[derive(Debug, Clone)]
pub enum ClientCommand {
    SendMessage {
        content: String,
    },
    EditMessage {
        message_id: MessageId,
        new_content: String,
    },
    ExecuteBashCommand {
        command: String,
    },
    ApproveToolCall {
        request_id: RequestId,
        decision: ApprovalDecision,
    },
    Cancel,
    RequestWorkspaceFiles,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Deny,
    Once,
    AlwaysTool,
    AlwaysBashPattern(String),
}
