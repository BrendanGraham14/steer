//! Core data model for TUI chat items
//!
//! This module defines the ChatItem enum which represents all possible rows
//! that can appear in the chat panel, including both conversation messages
//! and meta rows like slash commands and system notices.

use conductor_core::app::conversation::{AppCommandType, CommandResponse, Message};
use conductor_tools::ToolCall;
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

    /// Core replied to a command
    CoreCmdResponse {
        id: RowId,
        cmd: AppCommandType,
        resp: CommandResponse,
        ts: OffsetDateTime,
    },

    /// TUI command response (e.g., /help, /theme, /auth)
    TuiCommandResponse {
        id: RowId,
        command: String,
        response: String,
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
