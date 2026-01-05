use async_trait::async_trait;
use thiserror::Error;

use crate::app::domain::event::SessionEvent;
use crate::app::domain::types::SessionId;

#[derive(Debug, Error)]
pub enum EventStoreError {
    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("Database error: {message}")]
    Database { message: String },

    #[error("Serialization error: {message}")]
    Serialization { message: String },

    #[error("Connection error: {message}")]
    Connection { message: String },

    #[error("Migration error: {message}")]
    Migration { message: String },
}

impl EventStoreError {
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

    pub fn connection(message: impl Into<String>) -> Self {
        Self::Connection {
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(
        &self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> Result<u64, EventStoreError>;

    async fn load_events(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<(u64, SessionEvent)>, EventStoreError>;

    async fn load_events_after(
        &self,
        session_id: SessionId,
        after_seq: u64,
    ) -> Result<Vec<(u64, SessionEvent)>, EventStoreError>;

    async fn latest_sequence(&self, session_id: SessionId) -> Result<Option<u64>, EventStoreError>;

    async fn session_exists(&self, session_id: SessionId) -> Result<bool, EventStoreError>;

    async fn create_session(&self, session_id: SessionId) -> Result<(), EventStoreError>;

    async fn delete_session(&self, session_id: SessionId) -> Result<(), EventStoreError>;

    async fn list_session_ids(&self) -> Result<Vec<SessionId>, EventStoreError>;
}

pub struct InMemoryEventStore {
    events: std::sync::RwLock<std::collections::HashMap<SessionId, Vec<(u64, SessionEvent)>>>,
}

impl InMemoryEventStore {
    pub fn new() -> Self {
        Self {
            events: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for InMemoryEventStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventStore for InMemoryEventStore {
    async fn append(
        &self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> Result<u64, EventStoreError> {
        let mut events = self.events.write().unwrap();
        let session_events = events.entry(session_id).or_insert_with(Vec::new);

        let seq = session_events.last().map(|(s, _)| s + 1).unwrap_or(0);
        session_events.push((seq, event.clone()));
        Ok(seq)
    }

    async fn load_events(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<(u64, SessionEvent)>, EventStoreError> {
        let events = self.events.read().unwrap();
        Ok(events.get(&session_id).cloned().unwrap_or_default())
    }

    async fn load_events_after(
        &self,
        session_id: SessionId,
        after_seq: u64,
    ) -> Result<Vec<(u64, SessionEvent)>, EventStoreError> {
        let events = self.events.read().unwrap();
        Ok(events
            .get(&session_id)
            .map(|e| e.iter().filter(|(s, _)| *s > after_seq).cloned().collect())
            .unwrap_or_default())
    }

    async fn latest_sequence(&self, session_id: SessionId) -> Result<Option<u64>, EventStoreError> {
        let events = self.events.read().unwrap();
        Ok(events
            .get(&session_id)
            .and_then(|e| e.last().map(|(s, _)| *s)))
    }

    async fn session_exists(&self, session_id: SessionId) -> Result<bool, EventStoreError> {
        let events = self.events.read().unwrap();
        Ok(events.contains_key(&session_id))
    }

    async fn create_session(&self, session_id: SessionId) -> Result<(), EventStoreError> {
        let mut events = self.events.write().unwrap();
        events.entry(session_id).or_insert_with(Vec::new);
        Ok(())
    }

    async fn delete_session(&self, session_id: SessionId) -> Result<(), EventStoreError> {
        let mut events = self.events.write().unwrap();
        events.remove(&session_id);
        Ok(())
    }

    async fn list_session_ids(&self) -> Result<Vec<SessionId>, EventStoreError> {
        let events = self.events.read().unwrap();
        Ok(events.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::domain::event::SessionEvent;

    #[tokio::test]
    async fn test_in_memory_store_append_and_load() {
        let store = InMemoryEventStore::new();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let event = SessionEvent::Error {
            message: "test".to_string(),
        };

        let seq = store.append(session_id, &event).await.unwrap();
        assert_eq!(seq, 0);

        let events = store.load_events(session_id).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, 0);
    }

    #[tokio::test]
    async fn test_in_memory_store_sequence_numbers() {
        let store = InMemoryEventStore::new();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        for i in 0..5 {
            let event = SessionEvent::Error {
                message: format!("test {i}"),
            };
            let seq = store.append(session_id, &event).await.unwrap();
            assert_eq!(seq, i);
        }

        let latest = store.latest_sequence(session_id).await.unwrap();
        assert_eq!(latest, Some(4));
    }

    #[tokio::test]
    async fn test_in_memory_store_load_after_sequence() {
        let store = InMemoryEventStore::new();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        for i in 0..5 {
            let event = SessionEvent::Error {
                message: format!("test {i}"),
            };
            store.append(session_id, &event).await.unwrap();
        }

        let events = store.load_events_after(session_id, 2).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, 3);
        assert_eq!(events[1].0, 4);
    }

    #[tokio::test]
    async fn test_in_memory_store_session_isolation() {
        let store = InMemoryEventStore::new();
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
}
