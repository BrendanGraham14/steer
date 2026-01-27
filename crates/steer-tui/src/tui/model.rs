use crate::tui::core_commands::{CommandResponse as CoreCommandResponse, CoreCommandType};
use steer_grpc::client_api::{Message, McpServerInfo};
use steer_tools::ToolCall;
use time::OffsetDateTime;

pub type RowId = String;

/// Severity levels for system notices
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Error,
    Warn,
    Info,
}

/// All rows that can appear in the scrollback panel
#[derive(Debug, Clone)]
pub enum ChatItemData {
    /// Conversation message
    Message(Message),

    /// A tool call that is in progress
    PendingToolCall {
        id: RowId,
        tool_call: ToolCall,
        ts: OffsetDateTime,
    },

    /// Raw slash command entered by the user
    SlashInput {
        id: RowId,
        raw: String,
        ts: OffsetDateTime,
    },

    /// Internal notices (errors, warnings, info)
    SystemNotice {
        id: RowId,
        level: NoticeLevel,
        text: String,
        ts: OffsetDateTime,
    },

    CoreCmdResponse {
        id: RowId,
        command: CoreCommandType,
        response: CoreCommandResponse,
        ts: OffsetDateTime,
    },

    /// TUI command response (e.g., /help, /theme, /auth)
    TuiCommandResponse {
        id: RowId,
        command: String,
        response: TuiCommandResponse,
        ts: OffsetDateTime,
    },

    InFlightOperation {
        id: RowId,
        operation_id: uuid::Uuid,
        label: String,
        ts: OffsetDateTime,
    },
}

#[derive(Debug, Clone)]
pub enum TuiCommandResponse {
    Text(String),
    Theme { name: String },
    ListThemes(Vec<String>),
    ListMcpServers(Vec<McpServerInfo>),
}

#[derive(Debug, Clone)]
pub enum CommandResponse {
    Core(CoreCommandResponse),
    Tui(TuiCommandResponse),
}
impl From<CoreCommandResponse> for CommandResponse {
    fn from(response: CoreCommandResponse) -> Self {
        CommandResponse::Core(response)
    }
}
impl From<TuiCommandResponse> for CommandResponse {
    fn from(response: TuiCommandResponse) -> Self {
        CommandResponse::Tui(response)
    }
}

#[derive(Debug, Clone)]
pub struct ChatItem {
    pub parent_chat_item_id: Option<RowId>,
    pub data: ChatItemData,
}

impl ChatItem {
    /// Get the unique identifier for this chat item
    pub fn id(&self) -> &str {
        match &self.data {
            ChatItemData::Message(row) => row.id(),
            ChatItemData::PendingToolCall { id, .. } => id,
            ChatItemData::SlashInput { id, .. } => id,
            ChatItemData::CoreCmdResponse { id, .. } => id,
            ChatItemData::SystemNotice { id, .. } => id,
            ChatItemData::TuiCommandResponse { id, .. } => id,
            ChatItemData::InFlightOperation { id, .. } => id,
        }
    }

    /// Get the timestamp for this chat item
    pub fn timestamp(&self) -> OffsetDateTime {
        match &self.data {
            ChatItemData::Message(message) => {
                OffsetDateTime::from_unix_timestamp(message.timestamp() as i64).unwrap()
            }
            ChatItemData::PendingToolCall { ts, .. } => *ts,
            ChatItemData::SlashInput { ts, .. } => *ts,
            ChatItemData::CoreCmdResponse { ts, .. } => *ts,
            ChatItemData::SystemNotice { ts, .. } => *ts,
            ChatItemData::TuiCommandResponse { ts, .. } => *ts,
            ChatItemData::InFlightOperation { ts, .. } => *ts,
        }
    }

    /// Get the parent message ID for filtering
    pub fn parent_message_id(&self) -> Option<&str> {
        match &self.data {
            ChatItemData::Message(msg) => msg.parent_message_id(),
            _ => None,
        }
    }
}

pub fn generate_row_id() -> RowId {
    ulid::Ulid::new().to_string()
}
