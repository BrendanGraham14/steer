use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::app::domain::types::SessionId;
use crate::session::state::SessionConfig;

#[derive(Debug, Error)]
pub enum SessionCatalogError {
    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("Database error: {message}")]
    Database { message: String },

    #[error("Serialization error: {message}")]
    Serialization { message: String },
}

impl SessionCatalogError {
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
pub trait SessionCatalog: Send + Sync {
    async fn get_session_config(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionConfig>, SessionCatalogError>;

    async fn get_session_summary(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionSummary>, SessionCatalogError>;

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionSummary>, SessionCatalogError>;

    async fn update_session_catalog(
        &self,
        session_id: SessionId,
        config: Option<&SessionConfig>,
        increment_message_count: bool,
        new_model: Option<&str>,
    ) -> Result<(), SessionCatalogError>;
}
