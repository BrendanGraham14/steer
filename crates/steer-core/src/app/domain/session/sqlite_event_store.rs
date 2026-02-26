use async_trait::async_trait;
use base64::Engine as _;
use chrono::{DateTime, NaiveDateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{
    Row,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
    },
};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use super::event_store::{EventStore, EventStoreError};
use super::metadata_store::{
    SessionFilter, SessionMetadataStore, SessionMetadataStoreError, SessionSummary,
};
use crate::app::conversation::{
    AssistantContent, ImageContent, ImageSource, Message, MessageData, UserContent,
};
use crate::app::domain::event::SessionEvent;
use crate::app::domain::types::SessionId;
use crate::session::state::SessionConfig;
use steer_tools::tools::todo::TodoItem;

pub struct SqliteEventStore {
    pool: SqlitePool,
    media_root: Option<PathBuf>,
}

impl SqliteEventStore {
    pub async fn new(path: &Path) -> Result<Self, EventStoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                EventStoreError::connection(format!("Failed to create directory: {e}"))
            })?;
        }

        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .map_err(|e| EventStoreError::connection(format!("Invalid SQLite path: {e}")))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|e| {
                EventStoreError::connection(format!("Failed to connect to SQLite: {e}"))
            })?;

        let store = Self {
            pool,
            media_root: media_root_for_path(path),
        };
        store.run_migrations().await?;

        Ok(store)
    }

    pub async fn new_in_memory() -> Result<Self, EventStoreError> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(|e| EventStoreError::connection(format!("Invalid SQLite path: {e}")))?
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|e| {
                EventStoreError::connection(format!("Failed to connect to SQLite: {e}"))
            })?;

        let store = Self {
            pool,
            media_root: None,
        };
        store.run_migrations().await?;

        Ok(store)
    }

    async fn run_migrations(&self) -> Result<(), EventStoreError> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS domain_sessions (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                config_json TEXT,
                message_count INTEGER NOT NULL DEFAULT 0,
                last_model TEXT,
                title TEXT
            )
            ",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::Migration {
            message: format!("Failed to create sessions table: {e}"),
        })?;

        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS domain_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                sequence_num INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                event_data TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (session_id) REFERENCES domain_sessions(id) ON DELETE CASCADE,
                UNIQUE(session_id, sequence_num)
            )
            ",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::Migration {
            message: format!("Failed to create events table: {e}"),
        })?;

        sqlx::query(
            r"
            CREATE INDEX IF NOT EXISTS idx_domain_events_session_seq 
            ON domain_events(session_id, sequence_num)
            ",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::Migration {
            message: format!("Failed to create index: {e}"),
        })?;

        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS session_todos (
                session_id TEXT PRIMARY KEY,
                todos_json TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (session_id) REFERENCES domain_sessions(id) ON DELETE CASCADE
            )
            ",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::Migration {
            message: format!("Failed to create todos table: {e}"),
        })?;

        self.migrate_add_catalog_columns().await?;

        Ok(())
    }

    async fn migrate_add_catalog_columns(&self) -> Result<(), EventStoreError> {
        let has_updated_at: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('domain_sessions') WHERE name = 'updated_at'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        if !has_updated_at {
            sqlx::query("ALTER TABLE domain_sessions ADD COLUMN updated_at TEXT NOT NULL DEFAULT (datetime('now'))")
                .execute(&self.pool)
                .await
                .map_err(|e| EventStoreError::Migration {
                    message: format!("Failed to add updated_at column: {e}"),
                })?;

            sqlx::query("ALTER TABLE domain_sessions ADD COLUMN config_json TEXT")
                .execute(&self.pool)
                .await
                .map_err(|e| EventStoreError::Migration {
                    message: format!("Failed to add config_json column: {e}"),
                })?;

            sqlx::query(
                "ALTER TABLE domain_sessions ADD COLUMN message_count INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await
            .map_err(|e| EventStoreError::Migration {
                message: format!("Failed to add message_count column: {e}"),
            })?;

            sqlx::query("ALTER TABLE domain_sessions ADD COLUMN last_model TEXT")
                .execute(&self.pool)
                .await
                .map_err(|e| EventStoreError::Migration {
                    message: format!("Failed to add last_model column: {e}"),
                })?;
        }

        let has_title: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('domain_sessions') WHERE name = 'title'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        if !has_title {
            sqlx::query("ALTER TABLE domain_sessions ADD COLUMN title TEXT")
                .execute(&self.pool)
                .await
                .map_err(|e| EventStoreError::Migration {
                    message: format!("Failed to add title column: {e}"),
                })?;
        }

        Ok(())
    }

    fn prepare_event_for_storage(
        &self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> Result<SessionEvent, EventStoreError> {
        let Some(media_root) = &self.media_root else {
            return Ok(event.clone());
        };

        let session_root = media_root.join(session_id.to_string());

        match event {
            SessionEvent::UserMessageAdded { message } => {
                let message = Self::persist_message_images(session_id, message, &session_root)?;
                Ok(SessionEvent::UserMessageAdded { message })
            }
            SessionEvent::MessageUpdated { message } => {
                let message = Self::persist_message_images(session_id, message, &session_root)?;
                Ok(SessionEvent::MessageUpdated { message })
            }
            SessionEvent::AssistantMessageAdded { message, model } => {
                let message = Self::persist_message_images(session_id, message, &session_root)?;
                Ok(SessionEvent::AssistantMessageAdded {
                    message,
                    model: model.clone(),
                })
            }
            _ => Ok(event.clone()),
        }
    }

    fn persist_message_images(
        session_id: SessionId,
        message: &Message,
        session_root: &Path,
    ) -> Result<Message, EventStoreError> {
        let data = match &message.data {
            MessageData::User { content } => {
                let mut updated = Vec::with_capacity(content.len());
                let mut changed = false;
                for block in content {
                    match block {
                        UserContent::Image { image } => {
                            let persisted = Self::persist_image_content(
                                session_id,
                                &message.id,
                                image,
                                session_root,
                            )?;
                            changed |= persisted != *image;
                            updated.push(UserContent::Image { image: persisted });
                        }
                        _ => updated.push(block.clone()),
                    }
                }
                if !changed {
                    return Ok(message.clone());
                }
                MessageData::User { content: updated }
            }
            MessageData::Assistant { content } => {
                let mut updated = Vec::with_capacity(content.len());
                let mut changed = false;
                for block in content {
                    match block {
                        AssistantContent::Image { image } => {
                            let persisted = Self::persist_image_content(
                                session_id,
                                &message.id,
                                image,
                                session_root,
                            )?;
                            changed |= persisted != *image;
                            updated.push(AssistantContent::Image { image: persisted });
                        }
                        _ => updated.push(block.clone()),
                    }
                }
                if !changed {
                    return Ok(message.clone());
                }
                MessageData::Assistant { content: updated }
            }
            MessageData::Tool { .. } => return Ok(message.clone()),
        };

        Ok(Message {
            timestamp: message.timestamp,
            id: message.id.clone(),
            parent_message_id: message.parent_message_id.clone(),
            data,
        })
    }

    fn persist_image_content(
        session_id: SessionId,
        message_id: &str,
        image: &ImageContent,
        session_root: &Path,
    ) -> Result<ImageContent, EventStoreError> {
        let data_url = match &image.source {
            ImageSource::DataUrl { data_url } => data_url,
            ImageSource::SessionFile { relative_path } => {
                validate_session_relative_path(session_id, relative_path)?;
                return Ok(image.clone());
            }
            ImageSource::Url { .. } => return Ok(image.clone()),
        };

        let (mime_type, decoded) = decode_data_url(data_url).map_err(|e| {
            EventStoreError::serialization(format!(
                "Failed to decode image data URL for session {session_id}: {e}"
            ))
        })?;

        if !image.mime_type.is_empty() && image.mime_type != mime_type {
            return Err(EventStoreError::serialization(format!(
                "Image MIME type mismatch for message {message_id}: metadata='{}' data_url='{}'",
                image.mime_type, mime_type
            )));
        }

        std::fs::create_dir_all(session_root).map_err(|e| {
            EventStoreError::database(format!(
                "Failed to create media directory for session {session_id}: {e}"
            ))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(&decoded);
        let digest = hasher.finalize();
        let digest_hex = hex::encode(digest);

        let extension = extension_for_mime_type(&mime_type);
        let file_name = format!("{digest_hex}.{extension}");
        let absolute_path = session_root.join(&file_name);
        if !absolute_path.exists() {
            std::fs::write(&absolute_path, &decoded).map_err(|e| {
                EventStoreError::database(format!(
                    "Failed to persist image for session {session_id}: {e}"
                ))
            })?;
        }

        let relative_path = format!("{}/{}", session_id, file_name);
        Ok(ImageContent {
            mime_type,
            source: ImageSource::SessionFile { relative_path },
            width: image.width,
            height: image.height,
            bytes: Some(decoded.len() as u64),
            sha256: Some(digest_hex),
        })
    }

    fn event_type_string(event: &SessionEvent) -> &'static str {
        match event {
            SessionEvent::SessionCreated { .. } => "session_created",
            SessionEvent::SessionConfigUpdated { .. } => "session_config_updated",
            SessionEvent::AssistantMessageAdded { .. } => "assistant_message_added",
            SessionEvent::UserMessageAdded { .. } => "user_message_added",
            SessionEvent::ToolMessageAdded { .. } => "tool_message_added",
            SessionEvent::MessageUpdated { .. } => "message_updated",
            SessionEvent::ToolCallStarted { .. } => "tool_call_started",
            SessionEvent::ToolCallCompleted { .. } => "tool_call_completed",
            SessionEvent::ToolCallFailed { .. } => "tool_call_failed",
            SessionEvent::ApprovalRequested { .. } => "approval_requested",
            SessionEvent::ApprovalDecided { .. } => "approval_decided",
            SessionEvent::OperationStarted { .. } => "operation_started",
            SessionEvent::OperationCompleted { .. } => "operation_completed",
            SessionEvent::OperationCancelled { .. } => "operation_cancelled",
            SessionEvent::CompactResult { .. } => "compact_result",
            SessionEvent::ConversationCompacted { .. } => "conversation_compacted",
            SessionEvent::WorkspaceChanged => "workspace_changed",
            SessionEvent::QueueUpdated { .. } => "queue_updated",
            SessionEvent::Error { .. } => "error",
            SessionEvent::McpServerStateChanged { .. } => "mcp_server_state_changed",
            SessionEvent::LlmUsageUpdated { .. } => "llm_usage_updated",
        }
    }
}

fn parse_catalog_timestamp(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S%.f")
                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
        })
        .or_else(|_| {
            NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S")
                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
        })
        .unwrap_or_else(|_| Utc::now())
}

fn catalog_update_for_event(
    event: &SessionEvent,
) -> (Option<&SessionConfig>, bool, Option<String>) {
    let config = match event {
        SessionEvent::SessionCreated { config, .. }
        | SessionEvent::SessionConfigUpdated { config, .. } => Some(config.as_ref()),
        _ => None,
    };

    let increment_message_count = matches!(
        event,
        SessionEvent::AssistantMessageAdded { .. }
            | SessionEvent::UserMessageAdded { .. }
            | SessionEvent::ToolMessageAdded { .. }
    );

    let new_model = match event {
        SessionEvent::AssistantMessageAdded { model, .. }
        | SessionEvent::ToolCallStarted { model, .. }
        | SessionEvent::ToolCallCompleted { model, .. }
        | SessionEvent::ToolCallFailed { model, .. }
        | SessionEvent::LlmUsageUpdated { model, .. } => Some(model.to_string()),
        SessionEvent::ConversationCompacted { record } => Some(record.model.clone()),
        _ => None,
    };

    (config, increment_message_count, new_model)
}

#[async_trait]
impl EventStore for SqliteEventStore {
    async fn append(
        &self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> Result<u64, EventStoreError> {
        let session_id_str = session_id.0.to_string();
        let prepared_event = self.prepare_event_for_storage(session_id, event)?;
        let event_type = Self::event_type_string(&prepared_event);
        let event_data = serde_json::to_string(&prepared_event).map_err(|e| {
            EventStoreError::serialization(format!("Failed to serialize event: {e}"))
        })?;

        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_num), -1) + 1 FROM domain_events WHERE session_id = ?1",
        )
        .bind(&session_id_str)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| EventStoreError::database(format!("Failed to get next sequence: {e}")))?;

        sqlx::query(
            r"
            INSERT INTO domain_events (session_id, sequence_num, event_type, event_data)
            VALUES (?1, ?2, ?3, ?4)
            ",
        )
        .bind(&session_id_str)
        .bind(next_seq)
        .bind(event_type)
        .bind(&event_data)
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::database(format!("Failed to append event: {e}")))?;

        let (config, increment_message_count, new_model) =
            catalog_update_for_event(&prepared_event);
        if config.is_some() || increment_message_count || new_model.is_some() {
            self.update_session_metadata(
                session_id,
                config,
                increment_message_count,
                new_model.as_deref(),
            )
            .await
            .map_err(|e| {
                EventStoreError::database(format!(
                    "Failed to update session catalog after event append: {e}"
                ))
            })?;
        }

        Ok(next_seq as u64)
    }

    async fn load_events(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<(u64, SessionEvent)>, EventStoreError> {
        let session_id_str = session_id.0.to_string();

        let rows = sqlx::query(
            r"
            SELECT sequence_num, event_data
            FROM domain_events
            WHERE session_id = ?1
            ORDER BY sequence_num ASC
            ",
        )
        .bind(&session_id_str)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| EventStoreError::database(format!("Failed to load events: {e}")))?;

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let seq: i64 = row.get("sequence_num");
            let event_data: String = row.get("event_data");
            let event: SessionEvent = serde_json::from_str(&event_data)
                .map_err(|e| EventStoreError::serialization(format!("Invalid event data: {e}")))?;
            events.push((seq as u64, event));
        }

        Ok(events)
    }

    async fn load_events_after(
        &self,
        session_id: SessionId,
        after_seq: u64,
    ) -> Result<Vec<(u64, SessionEvent)>, EventStoreError> {
        let session_id_str = session_id.0.to_string();

        let rows = sqlx::query(
            r"
            SELECT sequence_num, event_data
            FROM domain_events
            WHERE session_id = ?1 AND sequence_num > ?2
            ORDER BY sequence_num ASC
            ",
        )
        .bind(&session_id_str)
        .bind(after_seq as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| EventStoreError::database(format!("Failed to load events: {e}")))?;

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let seq: i64 = row.get("sequence_num");
            let event_data: String = row.get("event_data");
            let event: SessionEvent = serde_json::from_str(&event_data)
                .map_err(|e| EventStoreError::serialization(format!("Invalid event data: {e}")))?;
            events.push((seq as u64, event));
        }

        Ok(events)
    }

    async fn latest_sequence(&self, session_id: SessionId) -> Result<Option<u64>, EventStoreError> {
        let session_id_str = session_id.0.to_string();

        let result: Option<i64> =
            sqlx::query_scalar("SELECT MAX(sequence_num) FROM domain_events WHERE session_id = ?1")
                .bind(&session_id_str)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    EventStoreError::database(format!("Failed to get latest sequence: {e}"))
                })?;

        Ok(result.map(|s| s as u64))
    }

    async fn session_exists(&self, session_id: SessionId) -> Result<bool, EventStoreError> {
        let session_id_str = session_id.0.to_string();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM domain_sessions WHERE id = ?1")
            .bind(&session_id_str)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| EventStoreError::database(format!("Failed to check session: {e}")))?;

        Ok(count > 0)
    }

    async fn create_session(&self, session_id: SessionId) -> Result<(), EventStoreError> {
        let session_id_str = session_id.0.to_string();

        sqlx::query("INSERT INTO domain_sessions (id) VALUES (?1)")
            .bind(&session_id_str)
            .execute(&self.pool)
            .await
            .map_err(|e| EventStoreError::database(format!("Failed to create session: {e}")))?;

        Ok(())
    }

    async fn delete_session(&self, session_id: SessionId) -> Result<(), EventStoreError> {
        let session_id_str = session_id.0.to_string();

        sqlx::query("DELETE FROM domain_events WHERE session_id = ?1")
            .bind(&session_id_str)
            .execute(&self.pool)
            .await
            .map_err(|e| EventStoreError::database(format!("Failed to delete events: {e}")))?;

        sqlx::query("DELETE FROM domain_sessions WHERE id = ?1")
            .bind(&session_id_str)
            .execute(&self.pool)
            .await
            .map_err(|e| EventStoreError::database(format!("Failed to delete session: {e}")))?;

        Ok(())
    }

    async fn list_session_ids(&self) -> Result<Vec<SessionId>, EventStoreError> {
        let rows = sqlx::query("SELECT id FROM domain_sessions ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| EventStoreError::database(format!("Failed to list sessions: {e}")))?;

        let mut session_ids = Vec::with_capacity(rows.len());
        for row in rows {
            let id_str: String = row.get("id");
            let uuid = uuid::Uuid::parse_str(&id_str)
                .map_err(|e| EventStoreError::serialization(format!("Invalid session ID: {e}")))?;
            session_ids.push(SessionId(uuid));
        }

        Ok(session_ids)
    }

    async fn load_todos(
        &self,
        session_id: SessionId,
    ) -> Result<Option<Vec<TodoItem>>, EventStoreError> {
        let session_id_str = session_id.0.to_string();

        let row = sqlx::query("SELECT todos_json FROM session_todos WHERE session_id = ?1")
            .bind(&session_id_str)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| EventStoreError::database(format!("Failed to load todos: {e}")))?;

        let Some(row) = row else {
            return Ok(None);
        };

        let todos_json: String = row
            .try_get("todos_json")
            .map_err(|e| EventStoreError::database(format!("Failed to read todos row: {e}")))?;

        let todos: Vec<TodoItem> = serde_json::from_str(&todos_json)
            .map_err(|e| EventStoreError::serialization(format!("Invalid todos JSON: {e}")))?;

        Ok(Some(todos))
    }

    async fn save_todos(
        &self,
        session_id: SessionId,
        todos: &[TodoItem],
    ) -> Result<(), EventStoreError> {
        let session_id_str = session_id.0.to_string();
        let todos_json = serde_json::to_string(todos).map_err(|e| {
            EventStoreError::serialization(format!("Failed to serialize todos: {e}"))
        })?;

        sqlx::query(
            r"
            INSERT INTO session_todos (session_id, todos_json, updated_at)
            VALUES (?1, ?2, datetime('now'))
            ON CONFLICT(session_id) DO UPDATE SET
                todos_json = excluded.todos_json,
                updated_at = datetime('now')
            ",
        )
        .bind(&session_id_str)
        .bind(&todos_json)
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::database(format!("Failed to save todos: {e}")))?;

        Ok(())
    }
}

#[async_trait]
impl SessionMetadataStore for SqliteEventStore {
    async fn get_session_config(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionConfig>, SessionMetadataStoreError> {
        let session_id_str = session_id.0.to_string();

        let row = sqlx::query("SELECT config_json FROM domain_sessions WHERE id = ?1")
            .bind(&session_id_str)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                SessionMetadataStoreError::database(format!("Failed to get session: {e}"))
            })?;

        match row {
            Some(row) => {
                let config_json: Option<String> = row.get("config_json");
                match config_json {
                    Some(json) => {
                        let config: SessionConfig = serde_json::from_str(&json).map_err(|e| {
                            SessionMetadataStoreError::serialization(format!(
                                "Failed to parse config: {e}"
                            ))
                        })?;
                        Ok(Some(config))
                    }
                    None => Ok(None),
                }
            }
            None => Ok(None),
        }
    }

    async fn get_session_summary(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionSummary>, SessionMetadataStoreError> {
        let session_id_str = session_id.0.to_string();

        let row = sqlx::query(
            "SELECT id, created_at, updated_at, message_count, last_model, title FROM domain_sessions WHERE id = ?1",
        )
        .bind(&session_id_str)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SessionMetadataStoreError::database(format!("Failed to get session: {e}")))?;

        match row {
            Some(row) => {
                let id_str: String = row.get("id");
                let created_at_str: String = row.get("created_at");
                let updated_at_str: String = row.get("updated_at");
                let message_count: i64 = row.get("message_count");
                let last_model: Option<String> = row.get("last_model");
                let title: Option<String> = row.get("title");

                let uuid = uuid::Uuid::parse_str(&id_str).map_err(|e| {
                    SessionMetadataStoreError::serialization(format!("Invalid session ID: {e}"))
                })?;

                let created_at = parse_catalog_timestamp(&created_at_str);
                let updated_at = parse_catalog_timestamp(&updated_at_str);

                Ok(Some(SessionSummary {
                    id: SessionId(uuid),
                    created_at,
                    updated_at,
                    message_count: message_count as u32,
                    last_model,
                    title,
                }))
            }
            None => Ok(None),
        }
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionSummary>, SessionMetadataStoreError> {
        let limit = filter.limit.unwrap_or(100) as i64;
        let offset = filter.offset.unwrap_or(0) as i64;

        let rows = sqlx::query(
            r"
            SELECT id, created_at, updated_at, message_count, last_model, title 
            FROM domain_sessions 
            ORDER BY updated_at DESC
            LIMIT ?1 OFFSET ?2
            ",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            SessionMetadataStoreError::database(format!("Failed to list sessions: {e}"))
        })?;

        let mut summaries = Vec::with_capacity(rows.len());
        for row in rows {
            let id_str: String = row.get("id");
            let created_at_str: String = row.get("created_at");
            let updated_at_str: String = row.get("updated_at");
            let message_count: i64 = row.get("message_count");
            let last_model: Option<String> = row.get("last_model");
            let title: Option<String> = row.get("title");

            let uuid = uuid::Uuid::parse_str(&id_str).map_err(|e| {
                SessionMetadataStoreError::serialization(format!("Invalid session ID: {e}"))
            })?;

            let created_at = parse_catalog_timestamp(&created_at_str);
            let updated_at = parse_catalog_timestamp(&updated_at_str);

            summaries.push(SessionSummary {
                id: SessionId(uuid),
                created_at,
                updated_at,
                message_count: message_count as u32,
                last_model,
                title,
            });
        }

        Ok(summaries)
    }

    async fn update_session_metadata(
        &self,
        session_id: SessionId,
        config: Option<&SessionConfig>,
        increment_message_count: bool,
        new_model: Option<&str>,
    ) -> Result<(), SessionMetadataStoreError> {
        let session_id_str = session_id.0.to_string();
        let now = Utc::now().to_rfc3339();

        if let Some(cfg) = config {
            let config_json = serde_json::to_string(cfg).map_err(|e| {
                SessionMetadataStoreError::serialization(format!("Failed to serialize config: {e}"))
            })?;

            sqlx::query(
                "UPDATE domain_sessions SET config_json = ?1, title = ?2, updated_at = ?3 WHERE id = ?4",
            )
            .bind(&config_json)
            .bind(cfg.title.as_deref())
            .bind(&now)
            .bind(&session_id_str)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                SessionMetadataStoreError::database(format!(
                    "Failed to update session config/title: {e}"
                ))
            })?;
        }

        if increment_message_count {
            sqlx::query(
                "UPDATE domain_sessions SET message_count = message_count + 1, updated_at = ?1 WHERE id = ?2",
            )
            .bind(&now)
            .bind(&session_id_str)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                SessionMetadataStoreError::database(format!("Failed to increment message count: {e}"))
            })?;
        }

        if let Some(model) = new_model {
            sqlx::query(
                "UPDATE domain_sessions SET last_model = ?1, updated_at = ?2 WHERE id = ?3",
            )
            .bind(model)
            .bind(&now)
            .bind(&session_id_str)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                SessionMetadataStoreError::database(format!("Failed to update last model: {e}"))
            })?;
        }

        Ok(())
    }
}

fn media_root_for_path(path: &Path) -> Option<PathBuf> {
    path.parent().map(|parent| parent.join("session_media"))
}

fn validate_session_relative_path(
    session_id: SessionId,
    relative_path: &str,
) -> Result<(), EventStoreError> {
    if relative_path.is_empty() {
        return Err(EventStoreError::serialization(
            "Session file path cannot be empty",
        ));
    }
    if relative_path.starts_with('/') || relative_path.starts_with('\\') {
        return Err(EventStoreError::serialization(format!(
            "Session file path must be relative: {relative_path}"
        )));
    }
    if relative_path.contains("..") {
        return Err(EventStoreError::serialization(format!(
            "Session file path cannot contain '..': {relative_path}"
        )));
    }

    let expected_prefix = format!("{}/", session_id);
    if !relative_path.starts_with(&expected_prefix) {
        return Err(EventStoreError::serialization(format!(
            "Session file path must be scoped to session {session_id}: {relative_path}"
        )));
    }

    Ok(())
}

fn extension_for_mime_type(mime_type: &str) -> &'static str {
    match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/heic" => "heic",
        "image/heif" => "heif",
        _ => "bin",
    }
}

fn decode_data_url(data_url: &str) -> Result<(String, Vec<u8>), String> {
    let Some((meta, payload)) = data_url.split_once(',') else {
        return Err("missing data URL separator ','".to_string());
    };

    let Some(meta) = meta.strip_prefix("data:") else {
        return Err("data URL must start with 'data:'".to_string());
    };

    let mut segments = meta.split(';');
    let mime_type = segments
        .next()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || "application/octet-stream".to_string(),
            |value| value.to_string(),
        );

    let mut is_base64 = false;
    for segment in segments {
        if segment.eq_ignore_ascii_case("base64") {
            is_base64 = true;
            break;
        }
    }

    if !is_base64 {
        return Err("only base64 data URLs are supported".to_string());
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| format!("invalid base64 payload: {e}"))?;
    Ok((mime_type, decoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::provider::TokenUsage;
    use crate::app::conversation::{
        AssistantContent, ImageContent, ImageSource, Message, MessageData, UserContent,
    };
    use crate::app::domain::event::{ContextWindowUsage, SessionEvent};
    use crate::app::domain::types::{OpId, ToolCallId};
    use crate::config::model::{ModelId, builtin};
    use crate::config::provider::ProviderId;
    use crate::session::state::{SessionConfig, ToolVisibility};
    use std::collections::{HashMap, HashSet};
    use steer_tools::error::{ToolError, ToolExecutionError, WorkspaceOpError};
    use steer_tools::result::ToolResult;
    use steer_tools::tools::read_file::ReadFileError;
    use steer_tools::tools::todo::{TodoItem, TodoPriority, TodoStatus};

    const PNG_1X1_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+X2N8AAAAASUVORK5CYII=";

    fn sample_todos() -> Vec<TodoItem> {
        vec![
            TodoItem {
                content: "first".to_string(),
                status: TodoStatus::Pending,
                priority: TodoPriority::High,
                id: "todo-1".to_string(),
            },
            TodoItem {
                content: "second".to_string(),
                status: TodoStatus::InProgress,
                priority: TodoPriority::Low,
                id: "todo-2".to_string(),
            },
        ]
    }

    fn sample_png_data_url() -> String {
        format!("data:image/png;base64,{PNG_1X1_BASE64}")
    }

    fn user_image_message(message_id: &str, image: ImageContent) -> Message {
        Message {
            timestamp: 0,
            id: message_id.to_string(),
            parent_message_id: None,
            data: MessageData::User {
                content: vec![UserContent::Image { image }],
            },
        }
    }

    fn user_text_message(message_id: &str, text: &str) -> Message {
        Message {
            timestamp: 0,
            id: message_id.to_string(),
            parent_message_id: None,
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: text.to_string(),
                }],
            },
        }
    }

    fn assistant_text_message(
        message_id: &str,
        text: &str,
        parent_message_id: Option<&str>,
    ) -> Message {
        Message {
            timestamp: 0,
            id: message_id.to_string(),
            parent_message_id: parent_message_id.map(std::string::ToString::to_string),
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: text.to_string(),
                }],
            },
        }
    }

    #[tokio::test]
    async fn test_sqlite_store_append_and_load() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let event = SessionEvent::Error {
            message: "test error".to_string(),
        };

        let seq = store.append(session_id, &event).await.unwrap();
        assert_eq!(seq, 0);

        let events = store.load_events(session_id).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, 0);

        match &events[0].1 {
            SessionEvent::Error { message } => assert_eq!(message, "test error"),
            _ => panic!("Expected Error event"),
        }
    }

    #[test]
    fn test_sqlite_event_type_mapping_for_llm_usage_updated() {
        let event = SessionEvent::LlmUsageUpdated {
            op_id: crate::app::domain::types::OpId::new(),
            model: builtin::claude_sonnet_4_5(),
            usage: TokenUsage::new(1, 2, 3),
            context_window: None,
        };

        assert_eq!(
            SqliteEventStore::event_type_string(&event),
            "llm_usage_updated"
        );
    }

    #[tokio::test]
    async fn test_sqlite_store_llm_usage_updated_roundtrip() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();
        store.create_session(session_id).await.unwrap();

        let op_id = OpId::new();
        let model = builtin::claude_sonnet_4_5();
        let usage = TokenUsage::new(17, 23, 40);
        let context_window = Some(ContextWindowUsage {
            max_context_tokens: Some(200_000),
            remaining_tokens: Some(199_960),
            utilization_ratio: Some(0.0002),
            estimated: false,
        });

        let event = SessionEvent::LlmUsageUpdated {
            op_id,
            model: model.clone(),
            usage,
            context_window: context_window.clone(),
        };

        store.append(session_id, &event).await.unwrap();
        let events = store.load_events(session_id).await.unwrap();

        let loaded_event = events.first().expect("event").1.clone();
        match loaded_event {
            SessionEvent::LlmUsageUpdated {
                op_id: loaded_op_id,
                model: loaded_model,
                usage: loaded_usage,
                context_window: loaded_context_window,
            } => {
                assert_eq!(loaded_op_id, op_id);
                assert_eq!(loaded_model, model);
                assert_eq!(loaded_usage, usage);
                assert_eq!(loaded_context_window, context_window);
            }
            other => panic!("Expected LlmUsageUpdated event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlite_store_tool_message_with_workspace_error_roundtrip() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let tool_error = ToolError::Execution(ToolExecutionError::ReadFile(
            ReadFileError::Workspace(WorkspaceOpError::NotFound),
        ));
        let message = Message {
            timestamp: 0,
            id: "tool-msg-1".to_string(),
            parent_message_id: None,
            data: MessageData::Tool {
                tool_use_id: "tool-call-1".to_string(),
                result: ToolResult::Error(tool_error),
            },
        };
        let event = SessionEvent::ToolMessageAdded { message };

        store.append(session_id, &event).await.unwrap();
        let events = store.load_events(session_id).await.unwrap();

        let loaded_event = events.first().expect("event").1.clone();
        match loaded_event {
            SessionEvent::ToolMessageAdded { message } => match message.data {
                MessageData::Tool { result, .. } => {
                    assert!(matches!(result, ToolResult::Error(_)));
                }
                other => panic!("Expected Tool message, got {other:?}"),
            },
            other => panic!("Expected ToolMessageAdded event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlite_store_tool_call_completed_with_workspace_error_roundtrip() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let tool_error = ToolError::Execution(ToolExecutionError::ReadFile(
            ReadFileError::Workspace(WorkspaceOpError::NotFound),
        ));
        let event = SessionEvent::ToolCallCompleted {
            id: ToolCallId::new(),
            name: "read_file".to_string(),
            result: ToolResult::Error(tool_error),
            model: builtin::claude_sonnet_4_5(),
        };

        store.append(session_id, &event).await.unwrap();
        let events = store.load_events(session_id).await.unwrap();

        let loaded_event = events.first().expect("event").1.clone();
        match loaded_event {
            SessionEvent::ToolCallCompleted { result, .. } => {
                assert!(matches!(result, ToolResult::Error(_)));
            }
            other => panic!("Expected ToolCallCompleted event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlite_store_todos_roundtrip() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();
        let todos = sample_todos();

        store.create_session(session_id).await.unwrap();
        store.save_todos(session_id, &todos).await.unwrap();

        let loaded = store.load_todos(session_id).await.unwrap();
        assert_eq!(loaded, Some(todos));
    }

    #[tokio::test]
    async fn test_sqlite_store_todos_missing_is_none() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let loaded = store.load_todos(session_id).await.unwrap();
        assert_eq!(loaded, None);
    }

    #[tokio::test]
    async fn test_sqlite_store_todos_delete_session_clears() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();
        let todos = sample_todos();

        store.create_session(session_id).await.unwrap();
        store.save_todos(session_id, &todos).await.unwrap();
        store.delete_session(session_id).await.unwrap();

        let loaded = store.load_todos(session_id).await.unwrap();
        assert_eq!(loaded, None);
    }

    #[tokio::test]
    async fn test_sqlite_store_sequence_numbers() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        for i in 0..5 {
            let event = SessionEvent::Error {
                message: format!("error {i}"),
            };
            let seq = store.append(session_id, &event).await.unwrap();
            assert_eq!(seq, i);
        }

        let latest = store.latest_sequence(session_id).await.unwrap();
        assert_eq!(latest, Some(4));
    }

    #[tokio::test]
    async fn test_sqlite_store_load_after_sequence() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        for i in 0..5 {
            let event = SessionEvent::Error {
                message: format!("error {i}"),
            };
            store.append(session_id, &event).await.unwrap();
        }

        let events = store.load_events_after(session_id, 2).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, 3);
        assert_eq!(events[1].0, 4);
    }

    #[tokio::test]
    async fn test_sqlite_store_session_isolation() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_a = SessionId::new();
        let session_b = SessionId::new();

        store.create_session(session_a).await.unwrap();
        store.create_session(session_b).await.unwrap();

        let event_a = SessionEvent::Error {
            message: "session a".to_string(),
        };
        let event_b = SessionEvent::Error {
            message: "session b".to_string(),
        };

        store.append(session_a, &event_a).await.unwrap();
        store.append(session_b, &event_b).await.unwrap();

        let events_a = store.load_events(session_a).await.unwrap();
        let events_b = store.load_events(session_b).await.unwrap();

        assert_eq!(events_a.len(), 1);
        assert_eq!(events_b.len(), 1);
    }

    #[tokio::test]
    async fn test_sqlite_store_delete_session() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let event = SessionEvent::Error {
            message: "test".to_string(),
        };
        store.append(session_id, &event).await.unwrap();

        assert!(store.session_exists(session_id).await.unwrap());

        store.delete_session(session_id).await.unwrap();

        assert!(!store.session_exists(session_id).await.unwrap());
        let events = store.load_events(session_id).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_store_list_sessions() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_a = SessionId::new();
        let session_b = SessionId::new();

        store.create_session(session_a).await.unwrap();
        store.create_session(session_b).await.unwrap();

        let sessions = store.list_session_ids().await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&session_a) || sessions.contains(&session_b));
    }

    #[tokio::test]
    async fn test_sqlite_store_parses_sqlite_timestamps_in_catalog_queries() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let created_at_raw = "2026-02-18 19:44:07";
        let updated_at_raw = "2026-02-19 10:08:24.123";

        sqlx::query("UPDATE domain_sessions SET created_at = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(created_at_raw)
            .bind(updated_at_raw)
            .bind(session_id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();

        let expected_created = DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDateTime::parse_from_str(created_at_raw, "%Y-%m-%d %H:%M:%S").unwrap(),
            Utc,
        );
        let expected_updated = DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDateTime::parse_from_str(updated_at_raw, "%Y-%m-%d %H:%M:%S%.f").unwrap(),
            Utc,
        );

        let summary = store
            .get_session_summary(session_id)
            .await
            .unwrap()
            .expect("summary");
        assert_eq!(summary.created_at, expected_created);
        assert_eq!(summary.updated_at, expected_updated);

        let sessions = store
            .list_sessions(SessionFilter {
                limit: Some(10),
                offset: None,
            })
            .await
            .unwrap();

        let listed = sessions
            .into_iter()
            .find(|session| session.id == session_id)
            .expect("session in list");
        assert_eq!(listed.created_at, expected_created);
        assert_eq!(listed.updated_at, expected_updated);
    }

    #[tokio::test]
    async fn test_sqlite_store_updates_catalog_message_count_and_last_model_from_events() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        let created = SessionEvent::SessionCreated {
            config: Box::new(config),
            metadata: HashMap::new(),
            parent_session_id: None,
        };
        store.append(session_id, &created).await.unwrap();

        let assistant_model = ModelId::new(ProviderId::from("provider-a"), "model-a");
        let tool_model = ModelId::new(ProviderId::from("provider-b"), "model-b");

        store
            .append(
                session_id,
                &SessionEvent::UserMessageAdded {
                    message: user_text_message("user-msg", "hello"),
                },
            )
            .await
            .unwrap();

        store
            .append(
                session_id,
                &SessionEvent::AssistantMessageAdded {
                    message: assistant_text_message("assistant-msg", "hi", Some("user-msg")),
                    model: assistant_model,
                },
            )
            .await
            .unwrap();

        store
            .append(
                session_id,
                &SessionEvent::ToolCallStarted {
                    id: ToolCallId::new(),
                    name: "read_file".to_string(),
                    parameters: serde_json::json!({"path": "README.md"}),
                    model: tool_model.clone(),
                },
            )
            .await
            .unwrap();

        let summary = store
            .get_session_summary(session_id)
            .await
            .unwrap()
            .expect("summary");

        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.last_model, Some(tool_model.to_string()));
    }

    #[tokio::test]
    async fn test_sqlite_store_tracks_title_from_config_field() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let mut initial = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        initial.title = Some("Initial".to_string());

        store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    config: Box::new(initial),
                    metadata: HashMap::new(),
                    parent_session_id: None,
                },
            )
            .await
            .unwrap();

        let mut updated = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        updated.title = Some("Updated Title".to_string());

        store
            .append(
                session_id,
                &SessionEvent::SessionConfigUpdated {
                    config: Box::new(updated),
                    primary_agent_id: "normal".to_string(),
                },
            )
            .await
            .unwrap();

        let summary = store
            .get_session_summary(session_id)
            .await
            .unwrap()
            .expect("summary");
        assert_eq!(summary.title.as_deref(), Some("Updated Title"));

        let listed = store
            .list_sessions(SessionFilter {
                limit: Some(10),
                offset: Some(0),
            })
            .await
            .unwrap();
        let listed_summary = listed
            .into_iter()
            .find(|entry| entry.id == session_id)
            .expect("listed summary");
        assert_eq!(listed_summary.title.as_deref(), Some("Updated Title"));
    }

    #[tokio::test]
    async fn test_sqlite_store_serializes_tool_visibility_whitelist() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let mut session_config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        session_config.tool_config.visibility =
            ToolVisibility::Whitelist(HashSet::from(["view".to_string()]));

        let event = SessionEvent::SessionCreated {
            config: Box::new(session_config),
            metadata: HashMap::new(),
            parent_session_id: None,
        };

        store.append(session_id, &event).await.unwrap();

        let events = store.load_events(session_id).await.unwrap();
        let loaded_config = match &events[0].1 {
            SessionEvent::SessionCreated { config, .. } => config,
            other => panic!("Expected SessionCreated event, got {other:?}"),
        };

        match &loaded_config.tool_config.visibility {
            ToolVisibility::Whitelist(allowed) => {
                assert!(allowed.contains("view"));
            }
            other => panic!("Expected Whitelist visibility, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlite_store_updates_config_on_session_config_updated() {
        let store = SqliteEventStore::new_in_memory().await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.system_prompt = Some("updated".to_string());

        let event = SessionEvent::SessionConfigUpdated {
            config: Box::new(config.clone()),
            primary_agent_id: "plan".to_string(),
        };

        store.append(session_id, &event).await.unwrap();

        let loaded = store
            .get_session_config(session_id)
            .await
            .unwrap()
            .expect("config");
        assert_eq!(loaded.system_prompt, config.system_prompt);
    }

    #[tokio::test]
    async fn test_sqlite_store_persists_image_data_url_as_session_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("events.sqlite");
        let store = SqliteEventStore::new(&db_path).await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let data_url = sample_png_data_url();
        let (_, expected_bytes) = decode_data_url(&data_url).expect("valid png data URL");
        let image = ImageContent {
            mime_type: "image/png".to_string(),
            source: ImageSource::DataUrl {
                data_url: data_url.clone(),
            },
            width: Some(1),
            height: Some(1),
            bytes: None,
            sha256: None,
        };
        let event = SessionEvent::UserMessageAdded {
            message: user_image_message("user-msg-image", image),
        };

        store.append(session_id, &event).await.unwrap();

        let raw_event_json: String = sqlx::query_scalar(
            "SELECT event_data FROM domain_events WHERE session_id = ?1 AND sequence_num = 0",
        )
        .bind(session_id.to_string())
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert!(!raw_event_json.contains(PNG_1X1_BASE64));
        assert!(raw_event_json.contains("session_file"));

        let events = store.load_events(session_id).await.unwrap();
        assert_eq!(events.len(), 1);

        let persisted_image = match &events[0].1 {
            SessionEvent::UserMessageAdded { message } => match &message.data {
                MessageData::User { content } => match content.first() {
                    Some(UserContent::Image { image }) => image,
                    other => panic!("expected first user content image, got {other:?}"),
                },
                other => panic!("expected user message data, got {other:?}"),
            },
            other => panic!("expected UserMessageAdded, got {other:?}"),
        };

        let relative_path = match &persisted_image.source {
            ImageSource::SessionFile { relative_path } => relative_path,
            other => panic!("expected session_file image source, got {other:?}"),
        };
        let expected_prefix = format!("{session_id}/");
        assert!(relative_path.starts_with(&expected_prefix));

        let media_file_path = temp_dir.path().join("session_media").join(relative_path);
        let persisted_bytes = std::fs::read(&media_file_path).unwrap();
        assert_eq!(persisted_bytes, expected_bytes);
        assert_eq!(persisted_image.bytes, Some(expected_bytes.len() as u64));
        assert_eq!(persisted_image.width, Some(1));
        assert_eq!(persisted_image.height, Some(1));
    }

    #[tokio::test]
    async fn test_sqlite_store_rejects_cross_session_file_reference() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("events.sqlite");
        let store = SqliteEventStore::new(&db_path).await.unwrap();
        let session_id = SessionId::new();
        let other_session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let image = ImageContent {
            mime_type: "image/png".to_string(),
            source: ImageSource::SessionFile {
                relative_path: format!("{other_session_id}/some-image.png"),
            },
            width: None,
            height: None,
            bytes: None,
            sha256: None,
        };
        let event = SessionEvent::UserMessageAdded {
            message: user_image_message("user-msg-session-file", image),
        };

        let err = store.append(session_id, &event).await.unwrap_err();
        match err {
            EventStoreError::Serialization { message } => {
                assert!(message.contains("must be scoped to session"));
            }
            other => panic!("expected serialization error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlite_store_rejects_mime_mismatch_between_metadata_and_data_url() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("events.sqlite");
        let store = SqliteEventStore::new(&db_path).await.unwrap();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let image = ImageContent {
            mime_type: "image/jpeg".to_string(),
            source: ImageSource::DataUrl {
                data_url: sample_png_data_url(),
            },
            width: None,
            height: None,
            bytes: None,
            sha256: None,
        };
        let event = SessionEvent::UserMessageAdded {
            message: user_image_message("user-msg-mime-mismatch", image),
        };

        let err = store.append(session_id, &event).await.unwrap_err();
        match err {
            EventStoreError::Serialization { message } => {
                assert!(message.contains("MIME type mismatch"));
            }
            other => panic!("expected serialization error, got {other:?}"),
        }
    }
}
