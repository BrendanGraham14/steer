//! Core data model for TUI chat items
//!
//! This module defines the ChatItem enum which represents all possible rows
//! that can appear in the chat panel, including both conversation messages
//! and meta rows like slash commands and system notices.

use conductor_core::app::conversation::{
    AppCommandType, CommandResponse, Message,
};
use conductor_tools::ToolCall;
use time::OffsetDateTime;
use uuid::Uuid;

/// Unique, sortable row identifier (monotonic ULID string)
pub type RowId = String;

/// Severity levels for system notices
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Error,
    Warn,
    Info,
}

/// Extended message data with UI-specific information
#[derive(Debug, Clone)]
pub struct MessageRow {
    /// The core message from conductor-core
    pub inner: Message,
}

impl MessageRow {
    pub fn new(message: Message) -> Self {
        Self { inner: message }
    }

    pub fn id(&self) -> &str {
        self.inner.id()
    }

    pub fn thread_id(&self) -> &Uuid {
        self.inner.thread_id()
    }
}

/// All rows that can appear in the scrollback panel
#[derive(Debug, Clone)]
pub enum ChatItem {
    /// Real conversation message – always belongs to a branch
    Message(MessageRow),

    /// A tool call that is in progress
    PendingToolCall {
        id: RowId,
        tool_call: ToolCall,
        ts: OffsetDateTime,
    },

    /// Raw slash command entered by the user (never sent to an LLM)
    SlashInput {
        id: RowId,
        raw: String,
        ts: OffsetDateTime,
    },

    /// Core replied to a command
    CmdResponse {
        id: RowId,
        cmd: AppCommandType,
        resp: CommandResponse,
        ts: OffsetDateTime,
    },

    /// Internal notices (errors, warnings, info)
    SystemNotice {
        id: RowId,
        level: NoticeLevel,
        text: String,
        ts: OffsetDateTime,
    },
}

impl ChatItem {
    /// Get the unique identifier for this chat item
    pub fn id(&self) -> &str {
        match self {
            ChatItem::Message(row) => row.id(),
            ChatItem::PendingToolCall { id, .. } => id,
            ChatItem::SlashInput { id, .. } => id,
            ChatItem::CmdResponse { id, .. } => id,
            ChatItem::SystemNotice { id, .. } => id,
        }
    }

    /// Get the timestamp for this chat item
    pub fn timestamp(&self) -> OffsetDateTime {
        match self {
            ChatItem::Message(row) => {
                // Convert from chrono to time crate
                // For now, use current time as placeholder - this should be updated
                // when Message includes a timestamp field
                OffsetDateTime::now_utc()
            }
            ChatItem::PendingToolCall { ts, .. } => *ts,
            ChatItem::SlashInput { ts, .. } => *ts,
            ChatItem::CmdResponse { ts, .. } => *ts,
            ChatItem::SystemNotice { ts, .. } => *ts,
        }
    }

    /// Check if this is a conversation message
    pub fn is_message(&self) -> bool {
        matches!(self, ChatItem::Message(_))
    }

    /// Check if this is a meta row (not part of conversation)
    pub fn is_meta(&self) -> bool {
        !self.is_message()
    }
}

/// Helper function to generate a new row ID (ULID)
pub fn generate_row_id() -> RowId {
    ulid::Ulid::new().to_string()
}
