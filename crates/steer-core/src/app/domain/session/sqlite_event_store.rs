use async_trait::async_trait;
use sqlx::{
    Row,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
    },
};
use std::path::Path;
use std::str::FromStr;

use super::event_store::{EventStore, EventStoreError};
use crate::app::domain::event::SessionEvent;
use crate::app::domain::types::SessionId;

pub struct SqliteEventStore {
    pool: SqlitePool,
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

        let store = Self { pool };
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

        let store = Self { pool };
        store.run_migrations().await?;

        Ok(store)
    }

    async fn run_migrations(&self) -> Result<(), EventStoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS domain_sessions (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::Migration {
            message: format!("Failed to create sessions table: {e}"),
        })?;

        sqlx::query(
            r#"
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
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::Migration {
            message: format!("Failed to create events table: {e}"),
        })?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_domain_events_session_seq 
            ON domain_events(session_id, sequence_num)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::Migration {
            message: format!("Failed to create index: {e}"),
        })?;

        Ok(())
    }

    fn event_type_string(event: &SessionEvent) -> &'static str {
        match event {
            SessionEvent::MessageAdded { .. } => "message_added",
            SessionEvent::MessageUpdated { .. } => "message_updated",
            SessionEvent::ToolCallStarted { .. } => "tool_call_started",
            SessionEvent::ToolCallCompleted { .. } => "tool_call_completed",
            SessionEvent::ToolCallFailed { .. } => "tool_call_failed",
            SessionEvent::ApprovalRequested { .. } => "approval_requested",
            SessionEvent::ApprovalDecided { .. } => "approval_decided",
            SessionEvent::OperationStarted { .. } => "operation_started",
            SessionEvent::OperationCompleted { .. } => "operation_completed",
            SessionEvent::OperationCancelled { .. } => "operation_cancelled",
            SessionEvent::ModelChanged { .. } => "model_changed",
            SessionEvent::WorkspaceChanged => "workspace_changed",
            SessionEvent::Error { .. } => "error",
        }
    }
}

#[async_trait]
impl EventStore for SqliteEventStore {
    async fn append(
        &self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> Result<u64, EventStoreError> {
        let session_id_str = session_id.0.to_string();
        let event_type = Self::event_type_string(event);
        let event_data = serde_json::to_string(event).map_err(|e| {
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
            r#"
            INSERT INTO domain_events (session_id, sequence_num, event_type, event_data)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        )
        .bind(&session_id_str)
        .bind(next_seq)
        .bind(event_type)
        .bind(&event_data)
        .execute(&self.pool)
        .await
        .map_err(|e| EventStoreError::database(format!("Failed to append event: {e}")))?;

        Ok(next_seq as u64)
    }

    async fn load_events(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<(u64, SessionEvent)>, EventStoreError> {
        let session_id_str = session_id.0.to_string();

        let rows = sqlx::query(
            r#"
            SELECT sequence_num, event_data
            FROM domain_events
            WHERE session_id = ?1
            ORDER BY sequence_num ASC
            "#,
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
            r#"
            SELECT sequence_num, event_data
            FROM domain_events
            WHERE session_id = ?1 AND sequence_num > ?2
            ORDER BY sequence_num ASC
            "#,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::domain::event::SessionEvent;

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
}
