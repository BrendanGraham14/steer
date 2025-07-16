use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{Session, SessionConfig, SessionInfo};
use crate::app::Message;
use crate::events::StreamEvent;
use conductor_tools::ToolCall;

/// Database-agnostic session store trait
#[async_trait]
pub trait SessionStore: Send + Sync {
    // Session lifecycle
    async fn create_session(&self, config: SessionConfig) -> Result<Session, SessionStoreError>;
    async fn get_session(&self, session_id: &str) -> Result<Option<Session>, SessionStoreError>;
    async fn update_session(&self, session: &Session) -> Result<(), SessionStoreError>;
    async fn delete_session(&self, session_id: &str) -> Result<(), SessionStoreError>;
    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionInfo>, SessionStoreError>;

    // Message operations
    async fn append_message(
        &self,
        session_id: &str,
        message: &Message,
    ) -> Result<(), SessionStoreError>;
    async fn get_messages(
        &self,
        session_id: &str,
        after_sequence: Option<u32>,
    ) -> Result<Vec<Message>, SessionStoreError>;

    // Tool operations
    async fn create_tool_call(
        &self,
        session_id: &str,
        tool_call: &ToolCall,
    ) -> Result<(), SessionStoreError>;
    async fn update_tool_call(
        &self,
        tool_call_id: &str,
        update: ToolCallUpdate,
    ) -> Result<(), SessionStoreError>;
    async fn get_pending_tool_calls(
        &self,
        session_id: &str,
    ) -> Result<Vec<ToolCall>, SessionStoreError>;

    // Event streaming
    async fn append_event(
        &self,
        session_id: &str,
        event: &StreamEvent,
    ) -> Result<u64, SessionStoreError>;
    async fn get_events(
        &self,
        session_id: &str,
        after_sequence: u64,
        limit: Option<u32>,
    ) -> Result<Vec<(u64, StreamEvent)>, SessionStoreError>;
    async fn delete_events_before(
        &self,
        session_id: &str,
        before_sequence: u64,
    ) -> Result<u64, SessionStoreError>;

    // Active message tracking
    async fn update_active_message_id(
        &self,
        session_id: &str,
        message_id: Option<&str>,
    ) -> Result<(), SessionStoreError>;
}

/// Filter for listing sessions
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Filter by creation date range
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,

    /// Filter by last update date range
    pub updated_after: Option<DateTime<Utc>>,
    pub updated_before: Option<DateTime<Utc>>,

    /// Filter by metadata key-value pairs
    pub metadata_filters: HashMap<String, String>,

    /// Filter by session status
    pub status_filter: Option<SessionStatus>,

    /// Pagination
    pub limit: Option<u32>,
    pub offset: Option<u32>,

    /// Ordering
    pub order_by: SessionOrderBy,
    pub order_direction: OrderDirection,
}

/// Session status for filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Session has an active app instance running in a live environment
    Active,
    /// Session has no active app instance (environment down or app not running)
    Inactive,
}

/// Ordering options for session listing
#[derive(Debug, Clone, Default)]
pub enum SessionOrderBy {
    #[default]
    CreatedAt,
    UpdatedAt,
    MessageCount,
}

/// Order direction
#[derive(Debug, Clone, Default)]
pub enum OrderDirection {
    #[default]
    Descending,
    Ascending,
}

/// Tool call update operations
#[derive(Debug, Clone)]
pub struct ToolCallUpdate {
    pub status: Option<super::ToolCallStatus>,
    pub result: Option<super::ToolExecutionStats>,
    pub error: Option<String>,
}

impl ToolCallUpdate {
    pub fn set_status(status: super::ToolCallStatus) -> Self {
        Self {
            status: Some(status),
            result: None,
            error: None,
        }
    }

    pub fn set_result(result: super::ToolExecutionStats) -> Self {
        Self {
            status: Some(super::ToolCallStatus::Completed),
            result: Some(result),
            error: None,
        }
    }

    pub fn set_error(error: String) -> Self {
        Self {
            status: Some(super::ToolCallStatus::Failed {
                error: error.clone(),
            }),
            result: None,
            error: Some(error),
        }
    }
}

/// Pagination support for messages
#[derive(Debug, Clone)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub has_more: bool,
    pub next_cursor: Option<MessageCursor>,
}

/// Cursor for stable message pagination
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageCursor {
    pub sequence_num: u32,
    pub message_id: Option<String>,
}

/// Archive store trait for cold storage
#[async_trait]
pub trait ArchiveStore: Send + Sync {
    async fn archive_session(&self, session: &Session) -> Result<String, SessionStoreError>;
    async fn restore_session(&self, archive_id: &str) -> Result<Session, SessionStoreError>;
    async fn delete_archive(&self, archive_id: &str) -> Result<(), SessionStoreError>;
}

/// Session store error types
#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("Tool call not found: {tool_call_id}")]
    ToolCallNotFound { tool_call_id: String },

    #[error("Database error: {message}")]
    Database { message: String },

    #[error("Serialization error: {message}")]
    Serialization { message: String },

    #[error("Transaction error: {message}")]
    Transaction { message: String },

    #[error("Validation error: {message}")]
    Validation { message: String },

    #[error("Connection error: {message}")]
    Connection { message: String },

    #[error("Migration error: {message}")]
    Migration { message: String },

    #[error("Constraint violation: {message}")]
    ConstraintViolation { message: String },

    #[error("Internal error: {message}")]
    Internal { message: String },

    #[error("{entity} not found: {id}")]
    NotFound { entity: String, id: String },
}

impl SessionStoreError {
    pub fn database<S: Into<String>>(message: S) -> Self {
        Self::Database {
            message: message.into(),
        }
    }

    pub fn serialization<S: Into<String>>(message: S) -> Self {
        Self::Serialization {
            message: message.into(),
        }
    }

    pub fn transaction<S: Into<String>>(message: S) -> Self {
        Self::Transaction {
            message: message.into(),
        }
    }

    pub fn validation<S: Into<String>>(message: S) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    pub fn connection<S: Into<String>>(message: S) -> Self {
        Self::Connection {
            message: message.into(),
        }
    }

    pub fn internal<S: Into<String>>(message: S) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }
}

/// Extension trait for SessionStore with additional convenience methods
#[async_trait]
pub trait SessionStoreExt: SessionStore {
    /// Get messages with pagination support
    async fn get_messages_paginated(
        &self,
        session_id: &str,
        page_size: u32,
        cursor: Option<MessageCursor>,
    ) -> Result<MessagePage, SessionStoreError> {
        let after_sequence = cursor.map(|c| c.sequence_num);
        let messages = self.get_messages(session_id, after_sequence).await?;

        let has_more = messages.len() > page_size as usize;
        let messages = if has_more {
            messages.into_iter().take(page_size as usize).collect()
        } else {
            messages
        };

        let next_cursor = if has_more && !messages.is_empty() {
            Some(MessageCursor {
                sequence_num: messages.len() as u32,
                message_id: messages.last().map(|m| m.id().to_string()),
            })
        } else {
            None
        };

        Ok(MessagePage {
            messages,
            has_more,
            next_cursor,
        })
    }

    /// Archive a completed session
    async fn archive_session(
        &self,
        session_id: &str,
        archive_store: &dyn ArchiveStore,
    ) -> Result<String, SessionStoreError> {
        let session = self.get_session(session_id).await?.ok_or_else(|| {
            SessionStoreError::SessionNotFound {
                session_id: session_id.to_string(),
            }
        })?;

        let archive_id = archive_store.archive_session(&session).await?;
        self.delete_session(session_id).await?;

        Ok(archive_id)
    }
}

// Blanket implementation for all SessionStore implementors
impl<T: SessionStore + ?Sized> SessionStoreExt for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_filter_creation() {
        let filter = SessionFilter {
            limit: Some(10),
            order_by: SessionOrderBy::UpdatedAt,
            ..Default::default()
        };

        assert_eq!(filter.limit, Some(10));
        assert!(matches!(filter.order_by, SessionOrderBy::UpdatedAt));
    }

    #[test]
    fn test_tool_call_update() {
        let update = ToolCallUpdate::set_error("Test error".to_string());

        assert!(update.status.is_some());
        assert!(update.error.is_some());
        assert_eq!(update.error.unwrap(), "Test error");
    }

    #[test]
    fn test_message_cursor() {
        let cursor = MessageCursor {
            sequence_num: 5,
            message_id: Some("msg-123".to_string()),
        };

        assert_eq!(cursor.sequence_num, 5);
        assert_eq!(cursor.message_id.unwrap(), "msg-123");
    }
}
