use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::app::domain::types::SessionId;
use crate::session::state::SessionConfig;

#[derive(Debug, Error)]
pub enum SessionMetadataStoreError {
    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("Database error: {message}")]
    Database { message: String },

    #[error("Serialization error: {message}")]
    Serialization { message: String },

    #[error("In-memory catalog lock poisoned: {message}")]
    LockPoisoned { message: String },
}

impl SessionMetadataStoreError {
    pub fn database(message: impl Into<String>) -> Self {
        Self::Database {
            message: message.into(),
        }
    }

    pub fn serialization(message: impl Into<String>) -> Self {
        Self::Serialization {
            message: message.into(),
        }
    }

    pub fn lock_poisoned(message: impl Into<String>) -> Self {
        Self::LockPoisoned {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: u32,
    pub last_model: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[async_trait]
pub trait SessionMetadataStore: Send + Sync {
    async fn get_session_config(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionConfig>, SessionMetadataStoreError>;

    async fn get_session_summary(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionSummary>, SessionMetadataStoreError>;

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionSummary>, SessionMetadataStoreError>;

    async fn update_session_metadata(
        &self,
        session_id: SessionId,
        config: Option<&SessionConfig>,
        increment_message_count: bool,
        new_model: Option<&str>,
    ) -> Result<(), SessionMetadataStoreError>;
}
