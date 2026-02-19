use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::app::domain::event::SessionEvent;
use crate::app::domain::types::SessionId;
use crate::session::state::SessionConfig;
use steer_tools::tools::todo::TodoItem;

use super::metadata_store::{
    SessionFilter, SessionMetadataStore, SessionMetadataStoreError, SessionSummary,
};

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

    #[error("In-memory store lock poisoned: {message}")]
    LockPoisoned { message: String },
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

    pub fn lock_poisoned(message: impl Into<String>) -> Self {
        Self::LockPoisoned {
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

    async fn load_todos(
        &self,
        session_id: SessionId,
    ) -> Result<Option<Vec<TodoItem>>, EventStoreError>;

    async fn save_todos(
        &self,
        session_id: SessionId,
        todos: &[TodoItem],
    ) -> Result<(), EventStoreError>;
}

pub struct InMemoryEventStore {
    events: std::sync::RwLock<std::collections::HashMap<SessionId, Vec<(u64, SessionEvent)>>>,
    catalog: std::sync::RwLock<std::collections::HashMap<SessionId, InMemoryCatalogEntry>>,
    todos: std::sync::RwLock<std::collections::HashMap<SessionId, Vec<TodoItem>>>,
}

struct InMemoryCatalogEntry {
    config: SessionConfig,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    message_count: u32,
    last_model: Option<String>,
    title: Option<String>,
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

fn apply_catalog_update(
    entry: &mut InMemoryCatalogEntry,
    (config, increment_message_count, new_model): (Option<&SessionConfig>, bool, Option<String>),
) {
    let mut updated = false;

    if let Some(config) = config {
        entry.title.clone_from(&config.title);
        entry.config = config.clone();
        updated = true;
    }

    if increment_message_count {
        entry.message_count += 1;
        updated = true;
    }

    if let Some(model) = new_model {
        entry.last_model = Some(model);
        updated = true;
    }

    if updated {
        entry.updated_at = Utc::now();
    }
}

impl InMemoryEventStore {
    pub fn new() -> Self {
        Self {
            events: std::sync::RwLock::new(std::collections::HashMap::new()),
            catalog: std::sync::RwLock::new(std::collections::HashMap::new()),
            todos: std::sync::RwLock::new(std::collections::HashMap::new()),
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
        let mut events = self
            .events
            .write()
            .map_err(|_| EventStoreError::lock_poisoned("events"))?;
        let session_events = events.entry(session_id).or_default();

        let seq = session_events.last().map_or(0, |(s, _)| s + 1);
        session_events.push((seq, event.clone()));

        drop(events);

        let mut catalog = self
            .catalog
            .write()
            .map_err(|_| EventStoreError::lock_poisoned("catalog"))?;

        match event {
            SessionEvent::SessionCreated { config, .. } => {
                let now = Utc::now();
                catalog.insert(
                    session_id,
                    InMemoryCatalogEntry {
                        config: *config.clone(),
                        created_at: now,
                        updated_at: now,
                        message_count: 0,
                        last_model: None,
                        title: config.title.clone(),
                    },
                );
            }
            _ => {
                if let Some(entry) = catalog.get_mut(&session_id) {
                    let update = catalog_update_for_event(event);
                    apply_catalog_update(entry, update);
                }
            }
        }

        Ok(seq)
    }

    async fn load_events(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<(u64, SessionEvent)>, EventStoreError> {
        let events = self
            .events
            .read()
            .map_err(|_| EventStoreError::lock_poisoned("events"))?;
        Ok(events.get(&session_id).cloned().unwrap_or_default())
    }

    async fn load_events_after(
        &self,
        session_id: SessionId,
        after_seq: u64,
    ) -> Result<Vec<(u64, SessionEvent)>, EventStoreError> {
        let events = self
            .events
            .read()
            .map_err(|_| EventStoreError::lock_poisoned("events"))?;
        Ok(events
            .get(&session_id)
            .map(|e| e.iter().filter(|(s, _)| *s > after_seq).cloned().collect())
            .unwrap_or_default())
    }

    async fn latest_sequence(&self, session_id: SessionId) -> Result<Option<u64>, EventStoreError> {
        let events = self
            .events
            .read()
            .map_err(|_| EventStoreError::lock_poisoned("events"))?;
        Ok(events
            .get(&session_id)
            .and_then(|e| e.last().map(|(s, _)| *s)))
    }

    async fn session_exists(&self, session_id: SessionId) -> Result<bool, EventStoreError> {
        let events = self
            .events
            .read()
            .map_err(|_| EventStoreError::lock_poisoned("events"))?;
        Ok(events.contains_key(&session_id))
    }

    async fn create_session(&self, session_id: SessionId) -> Result<(), EventStoreError> {
        let mut events = self
            .events
            .write()
            .map_err(|_| EventStoreError::lock_poisoned("events"))?;
        events.entry(session_id).or_default();
        Ok(())
    }

    async fn delete_session(&self, session_id: SessionId) -> Result<(), EventStoreError> {
        let mut events = self
            .events
            .write()
            .map_err(|_| EventStoreError::lock_poisoned("events"))?;
        events.remove(&session_id);
        drop(events);

        let mut catalog = self
            .catalog
            .write()
            .map_err(|_| EventStoreError::lock_poisoned("catalog"))?;
        catalog.remove(&session_id);
        drop(catalog);

        let mut todos = self
            .todos
            .write()
            .map_err(|_| EventStoreError::lock_poisoned("todos"))?;
        todos.remove(&session_id);
        Ok(())
    }

    async fn list_session_ids(&self) -> Result<Vec<SessionId>, EventStoreError> {
        let events = self
            .events
            .read()
            .map_err(|_| EventStoreError::lock_poisoned("events"))?;
        Ok(events.keys().copied().collect())
    }

    async fn load_todos(
        &self,
        session_id: SessionId,
    ) -> Result<Option<Vec<TodoItem>>, EventStoreError> {
        let todos = self
            .todos
            .read()
            .map_err(|_| EventStoreError::lock_poisoned("todos"))?;
        Ok(todos.get(&session_id).cloned())
    }

    async fn save_todos(
        &self,
        session_id: SessionId,
        todos: &[TodoItem],
    ) -> Result<(), EventStoreError> {
        let mut store = self
            .todos
            .write()
            .map_err(|_| EventStoreError::lock_poisoned("todos"))?;
        store.insert(session_id, todos.to_vec());
        Ok(())
    }
}

#[async_trait]
impl SessionMetadataStore for InMemoryEventStore {
    async fn get_session_config(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionConfig>, SessionMetadataStoreError> {
        let catalog = self
            .catalog
            .read()
            .map_err(|_| SessionMetadataStoreError::lock_poisoned("catalog"))?;
        Ok(catalog.get(&session_id).map(|e| e.config.clone()))
    }

    async fn get_session_summary(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionSummary>, SessionMetadataStoreError> {
        let catalog = self
            .catalog
            .read()
            .map_err(|_| SessionMetadataStoreError::lock_poisoned("catalog"))?;
        Ok(catalog.get(&session_id).map(|e| SessionSummary {
            id: session_id,
            created_at: e.created_at,
            updated_at: e.updated_at,
            message_count: e.message_count,
            last_model: e.last_model.clone(),
            title: e.title.clone(),
        }))
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionSummary>, SessionMetadataStoreError> {
        let catalog = self
            .catalog
            .read()
            .map_err(|_| SessionMetadataStoreError::lock_poisoned("catalog"))?;
        let mut summaries: Vec<SessionSummary> = catalog
            .iter()
            .map(|(id, e)| SessionSummary {
                id: *id,
                created_at: e.created_at,
                updated_at: e.updated_at,
                message_count: e.message_count,
                last_model: e.last_model.clone(),
                title: e.title.clone(),
            })
            .collect();

        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        if let Some(offset) = filter.offset {
            summaries = summaries.into_iter().skip(offset).collect();
        }
        if let Some(limit) = filter.limit {
            summaries.truncate(limit);
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
        let mut catalog = self
            .catalog
            .write()
            .map_err(|_| SessionMetadataStoreError::lock_poisoned("catalog"))?;

        if let Some(entry) = catalog.get_mut(&session_id) {
            if let Some(cfg) = config {
                entry.config = cfg.clone();
                entry.title.clone_from(&cfg.title);
            }
            if increment_message_count {
                entry.message_count += 1;
            }
            if let Some(model) = new_model {
                entry.last_model = Some(model.to_string());
            }
            entry.updated_at = Utc::now();
        } else if let Some(cfg) = config {
            let now = Utc::now();
            catalog.insert(
                session_id,
                InMemoryCatalogEntry {
                    config: cfg.clone(),
                    created_at: now,
                    updated_at: now,
                    message_count: u32::from(increment_message_count),
                    last_model: new_model.map(String::from),
                    title: cfg.title.clone(),
                },
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::conversation::{AssistantContent, Message, MessageData, UserContent};
    use crate::app::domain::event::SessionEvent;
    use crate::app::domain::types::ToolCallId;
    use crate::config::model::{ModelId, builtin};
    use crate::config::provider::ProviderId;
    use crate::session::state::SessionConfig;
    use std::collections::HashMap;
    use steer_tools::tools::todo::{TodoItem, TodoPriority, TodoStatus};

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
                priority: TodoPriority::Medium,
                id: "todo-2".to_string(),
            },
        ]
    }

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
    async fn test_in_memory_store_updates_message_count_and_last_model_from_events() {
        let store = InMemoryEventStore::new();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.system_prompt = Some("initial".to_string());

        let created = SessionEvent::SessionCreated {
            config: Box::new(config),
            metadata: HashMap::new(),
            parent_session_id: None,
        };
        store.append(session_id, &created).await.unwrap();

        let user_message = Message {
            timestamp: 1,
            id: "user-msg".to_string(),
            parent_message_id: None,
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "hello".to_string(),
                }],
            },
        };

        let assistant_model = ModelId::new(ProviderId::from("provider-a"), "model-a");
        let assistant_message = Message {
            timestamp: 2,
            id: "assistant-msg".to_string(),
            parent_message_id: Some("user-msg".to_string()),
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "hi there".to_string(),
                }],
            },
        };

        let tool_model = ModelId::new(ProviderId::from("provider-b"), "model-b");

        store
            .append(
                session_id,
                &SessionEvent::UserMessageAdded {
                    message: user_message,
                },
            )
            .await
            .unwrap();

        store
            .append(
                session_id,
                &SessionEvent::AssistantMessageAdded {
                    message: assistant_message,
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
        assert_eq!(summary.title, None);
    }

    #[tokio::test]
    async fn test_in_memory_store_tracks_title_from_config_field() {
        let store = InMemoryEventStore::new();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.title = Some("Initial Title".to_string());

        store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    config: Box::new(config),
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
                    primary_agent_id: "plan".to_string(),
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
    }

    #[tokio::test]
    async fn test_in_memory_store_todos_roundtrip() {
        let store = InMemoryEventStore::new();
        let session_id = SessionId::new();
        let todos = sample_todos();

        store.save_todos(session_id, &todos).await.unwrap();

        let loaded = store.load_todos(session_id).await.unwrap();
        assert_eq!(loaded, Some(todos));
    }

    #[tokio::test]
    async fn test_in_memory_store_todos_isolation() {
        let store = InMemoryEventStore::new();
        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let todos = sample_todos();

        store.save_todos(session_a, &todos).await.unwrap();

        let loaded_a = store.load_todos(session_a).await.unwrap();
        let loaded_b = store.load_todos(session_b).await.unwrap();
        assert_eq!(loaded_a, Some(todos));
        assert_eq!(loaded_b, None);
    }

    #[tokio::test]
    async fn test_in_memory_store_delete_session_clears_todos() {
        let store = InMemoryEventStore::new();
        let session_id = SessionId::new();
        let todos = sample_todos();

        store.save_todos(session_id, &todos).await.unwrap();
        store.delete_session(session_id).await.unwrap();

        let loaded = store.load_todos(session_id).await.unwrap();
        assert_eq!(loaded, None);
    }

    #[tokio::test]
    async fn test_in_memory_store_updates_config_on_session_config_updated() {
        let store = InMemoryEventStore::new();
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();

        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.system_prompt = Some("initial".to_string());

        let created = SessionEvent::SessionCreated {
            config: Box::new(config.clone()),
            metadata: HashMap::new(),
            parent_session_id: None,
        };
        store.append(session_id, &created).await.unwrap();

        let mut updated = config.clone();
        updated.system_prompt = Some("updated".to_string());

        let event = SessionEvent::SessionConfigUpdated {
            config: Box::new(updated.clone()),
            primary_agent_id: "plan".to_string(),
        };
        store.append(session_id, &event).await.unwrap();

        let loaded = store
            .get_session_config(session_id)
            .await
            .unwrap()
            .expect("config");
        assert_eq!(loaded.system_prompt, updated.system_prompt);
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
