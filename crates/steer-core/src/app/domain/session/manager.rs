//! Session manager with LRU eviction for memory-efficient session handling.
//!
//! Sessions are loaded on-demand and evicted when idle or when memory limits are reached.
//! All session state is persisted via the EventStore, so eviction is safe.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lru::LruCache;
use thiserror::Error;

use super::event_store::{EventStore, EventStoreError};
use crate::app::domain::event::SessionEvent;
use crate::app::domain::reduce::apply_event_to_state;
use crate::app::domain::state::AppState;
use crate::app::domain::types::SessionId;
use crate::config::model::ModelId;

/// Errors that can occur during session management.
#[derive(Debug, Error)]
pub enum SessionManagerError {
    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("Event store error: {0}")]
    EventStore(#[from] EventStoreError),

    #[error("Session already exists: {session_id}")]
    SessionAlreadyExists { session_id: String },

    #[error("Invalid configuration: {message}")]
    InvalidConfig { message: String },
}

impl SessionManagerError {
    pub fn not_found(session_id: SessionId) -> Self {
        Self::SessionNotFound {
            session_id: session_id.to_string(),
        }
    }

    pub fn already_exists(session_id: SessionId) -> Self {
        Self::SessionAlreadyExists {
            session_id: session_id.to_string(),
        }
    }
}

/// Configuration for the session manager.
#[derive(Debug, Clone)]
pub struct SessionManagerConfig {
    /// Maximum number of active sessions in memory.
    pub max_active_sessions: usize,

    /// Duration after which an idle session may be evicted.
    pub idle_timeout: Duration,

    /// Default model for new sessions.
    pub default_model: ModelId,
}

impl SessionManagerConfig {
    /// Create a new configuration with sensible defaults.
    pub fn new(default_model: ModelId) -> Self {
        Self {
            max_active_sessions: 10,
            idle_timeout: Duration::from_secs(300), // 5 minutes
            default_model,
        }
    }

    /// Set the maximum number of active sessions.
    pub fn with_max_active(mut self, max: usize) -> Self {
        self.max_active_sessions = max;
        self
    }

    /// Set the idle timeout duration.
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }
}

/// An active session in memory.
#[derive(Debug)]
pub struct ActiveSession {
    /// The session ID.
    pub session_id: SessionId,

    /// The current state of the session.
    pub state: AppState,

    /// When the session was last accessed.
    pub last_activity: Instant,
}

impl ActiveSession {
    /// Create a new active session.
    pub fn new(session_id: SessionId, state: AppState) -> Self {
        Self {
            session_id,
            state,
            last_activity: Instant::now(),
        }
    }

    /// Mark the session as recently accessed.
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Check if the session has been idle longer than the given duration.
    pub fn is_idle(&self, timeout: Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }
}

/// Manages session lifecycle with LRU eviction.
///
/// Sessions are:
/// - Loaded on-demand from the EventStore
/// - Cached in memory for fast access
/// - Evicted when idle or when memory limits are reached
/// - Safe to evict because all state is persisted via events
pub struct SessionManager {
    /// Active sessions in memory (LRU cache).
    active: LruCache<SessionId, ActiveSession>,

    /// Event store for persistence.
    store: Arc<dyn EventStore>,

    /// Configuration.
    config: SessionManagerConfig,
}

impl SessionManager {
    /// Create a new session manager.
    pub fn new(
        store: Arc<dyn EventStore>,
        config: SessionManagerConfig,
    ) -> Result<Self, SessionManagerError> {
        if config.max_active_sessions == 0 {
            return Err(SessionManagerError::InvalidConfig {
                message: "max_active_sessions must be > 0".to_string(),
            });
        }

        let capacity = NonZeroUsize::new(config.max_active_sessions)
            .expect("max_active_sessions already validated");

        Ok(Self {
            active: LruCache::new(capacity),
            store,
            config,
        })
    }

    /// Get or load a session, returning a mutable reference to the active session.
    ///
    /// If the session is not in memory, it will be hydrated from the event store.
    /// If memory limits are exceeded, the least recently used session will be evicted.
    pub async fn get_session(
        &mut self,
        session_id: SessionId,
    ) -> Result<&mut ActiveSession, SessionManagerError> {
        if !self.active.contains(&session_id) {
            if !self.store.session_exists(session_id).await? {
                return Err(SessionManagerError::not_found(session_id));
            }

            if self.active.len() >= self.config.max_active_sessions {
                self.evict_lru();
            }

            let session = self.hydrate_session(session_id).await?;
            self.active.put(session_id, session);
        }

        let session = self
            .active
            .get_mut(&session_id)
            .expect("session inserted above");
        session.touch();
        Ok(session)
    }

    /// Get a session if it's already in memory, without loading from store.
    pub fn get_active(&mut self, session_id: SessionId) -> Option<&mut ActiveSession> {
        let session = self.active.get_mut(&session_id)?;
        session.touch();
        Some(session)
    }

    /// Check if a session is currently loaded in memory.
    pub fn is_loaded(&self, session_id: SessionId) -> bool {
        self.active.contains(&session_id)
    }

    /// Create a new session.
    ///
    /// This creates the session in the event store and optionally loads it into memory.
    pub async fn create_session(
        &mut self,
        session_id: SessionId,
    ) -> Result<&mut ActiveSession, SessionManagerError> {
        if self.store.session_exists(session_id).await? {
            return Err(SessionManagerError::already_exists(session_id));
        }

        self.store.create_session(session_id).await?;

        let state = AppState::new(session_id);
        let session = ActiveSession::new(session_id, state);

        if self.active.len() >= self.config.max_active_sessions {
            self.evict_lru();
        }

        self.active.put(session_id, session);

        Ok(self
            .active
            .get_mut(&session_id)
            .expect("session inserted above"))
    }

    pub async fn delete_session(
        &mut self,
        session_id: SessionId,
    ) -> Result<(), SessionManagerError> {
        self.active.pop(&session_id);
        self.store.delete_session(session_id).await?;
        Ok(())
    }

    /// Evict sessions that have been idle longer than the configured timeout.
    ///
    /// Returns the number of sessions evicted.
    pub fn evict_idle(&mut self) -> usize {
        let timeout = self.config.idle_timeout;
        let to_evict: Vec<SessionId> = self
            .active
            .iter()
            .filter(|(_, session)| session.is_idle(timeout))
            .map(|(id, _)| *id)
            .collect();

        let count = to_evict.len();
        for session_id in to_evict {
            self.active.pop(&session_id);
            tracing::debug!(session_id = %session_id, "Evicted idle session");
        }

        count
    }

    /// Force eviction of the least recently used session.
    ///
    /// Returns the evicted session ID, if any.
    pub fn evict_lru(&mut self) -> Option<SessionId> {
        if let Some((session_id, _)) = self.active.pop_lru() {
            tracing::debug!(session_id = %session_id, "Evicted LRU session");
            Some(session_id)
        } else {
            None
        }
    }

    /// Get the number of active sessions in memory.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Get the maximum capacity.
    pub fn capacity(&self) -> usize {
        self.config.max_active_sessions
    }

    /// List all session IDs from the event store.
    pub async fn list_all_sessions(&self) -> Result<Vec<SessionId>, SessionManagerError> {
        Ok(self.store.list_session_ids().await?)
    }

    async fn hydrate_session(
        &self,
        session_id: SessionId,
    ) -> Result<ActiveSession, SessionManagerError> {
        let events = self.store.load_events(session_id).await?;

        let mut state = AppState::new(session_id);

        for (_, event) in &events {
            apply_event_to_state(&mut state, event);
        }

        tracing::debug!(
            session_id = %session_id,
            event_count = events.len(),
            "Hydrated session"
        );

        Ok(ActiveSession::new(session_id, state))
    }

    pub async fn persist_event(
        &mut self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> Result<u64, SessionManagerError> {
        let seq = self.store.append(session_id, event).await?;

        if let Some(session) = self.active.get_mut(&session_id) {
            apply_event_to_state(&mut session.state, event);
            session.touch();
        }

        Ok(seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::domain::session::event_store::InMemoryEventStore;
    use crate::config::model::builtin;

    fn test_config() -> SessionManagerConfig {
        SessionManagerConfig::new(builtin::claude_sonnet_4_5())
            .with_max_active(3)
            .with_idle_timeout(Duration::from_millis(100))
    }

    #[tokio::test]
    async fn test_create_and_get_session() {
        let store = Arc::new(InMemoryEventStore::new());
        let mut manager = SessionManager::new(store, test_config()).unwrap();

        let session_id = SessionId::new();
        manager.create_session(session_id).await.unwrap();

        assert!(manager.is_loaded(session_id));
        assert_eq!(manager.active_count(), 1);

        let session = manager.get_session(session_id).await.unwrap();
        assert_eq!(session.session_id, session_id);
    }

    #[tokio::test]
    async fn test_session_not_found() {
        let store = Arc::new(InMemoryEventStore::new());
        let mut manager = SessionManager::new(store, test_config()).unwrap();

        let session_id = SessionId::new();
        let result = manager.get_session(session_id).await;

        assert!(matches!(
            result,
            Err(SessionManagerError::SessionNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn test_duplicate_session_creation() {
        let store = Arc::new(InMemoryEventStore::new());
        let mut manager = SessionManager::new(store, test_config()).unwrap();

        let session_id = SessionId::new();
        manager.create_session(session_id).await.unwrap();

        let result = manager.create_session(session_id).await;
        assert!(matches!(
            result,
            Err(SessionManagerError::SessionAlreadyExists { .. })
        ));
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let store = Arc::new(InMemoryEventStore::new());
        let mut manager = SessionManager::new(store, test_config()).unwrap();

        let session1 = SessionId::new();
        let session2 = SessionId::new();
        let session3 = SessionId::new();

        manager.create_session(session1).await.unwrap();
        manager.create_session(session2).await.unwrap();
        manager.create_session(session3).await.unwrap();

        assert_eq!(manager.active_count(), 3);

        let session4 = SessionId::new();
        manager.create_session(session4).await.unwrap();

        assert_eq!(manager.active_count(), 3);
        assert!(!manager.is_loaded(session1));
        assert!(manager.is_loaded(session4));
    }

    #[tokio::test]
    async fn test_idle_eviction() {
        let store = Arc::new(InMemoryEventStore::new());
        let config = SessionManagerConfig::new(builtin::claude_sonnet_4_5())
            .with_max_active(10)
            .with_idle_timeout(Duration::from_millis(10));
        let mut manager = SessionManager::new(store, config).unwrap();

        let session_id = SessionId::new();
        manager.create_session(session_id).await.unwrap();

        assert!(manager.is_loaded(session_id));

        tokio::time::sleep(Duration::from_millis(20)).await;

        let evicted = manager.evict_idle();
        assert_eq!(evicted, 1);
        assert!(!manager.is_loaded(session_id));
    }

    #[tokio::test]
    async fn test_hydration_from_store() {
        let store = Arc::new(InMemoryEventStore::new());
        let session_id = SessionId::new();

        store.create_session(session_id).await.unwrap();
        store
            .append(
                session_id,
                &SessionEvent::Error {
                    message: "test event 1".to_string(),
                },
            )
            .await
            .unwrap();
        store
            .append(
                session_id,
                &SessionEvent::Error {
                    message: "test event 2".to_string(),
                },
            )
            .await
            .unwrap();

        let mut manager = SessionManager::new(store, test_config()).unwrap();
        let session = manager.get_session(session_id).await.unwrap();

        assert_eq!(session.state.event_sequence, 2);
    }

    #[tokio::test]
    async fn test_delete_session() {
        let store = Arc::new(InMemoryEventStore::new());
        let mut manager = SessionManager::new(store.clone(), test_config()).unwrap();

        let session_id = SessionId::new();
        manager.create_session(session_id).await.unwrap();

        assert!(manager.is_loaded(session_id));
        assert!(store.session_exists(session_id).await.unwrap());

        manager.delete_session(session_id).await.unwrap();

        assert!(!manager.is_loaded(session_id));
        assert!(!store.session_exists(session_id).await.unwrap());
    }

    #[tokio::test]
    async fn test_persist_event_updates_memory() {
        let store = Arc::new(InMemoryEventStore::new());
        let mut manager = SessionManager::new(store, test_config()).unwrap();

        let session_id = SessionId::new();
        manager.create_session(session_id).await.unwrap();

        {
            let session = manager.get_active(session_id).unwrap();
            assert_eq!(session.state.event_sequence, 0);
        }

        let seq = manager
            .persist_event(
                session_id,
                &SessionEvent::Error {
                    message: "test".to_string(),
                },
            )
            .await
            .unwrap();

        assert_eq!(seq, 0);

        {
            let session = manager.get_active(session_id).unwrap();
            assert_eq!(session.state.event_sequence, 1);
        }
    }

    #[tokio::test]
    async fn test_reload_after_eviction() {
        let store = Arc::new(InMemoryEventStore::new());
        let mut manager = SessionManager::new(store, test_config()).unwrap();

        let session_id = SessionId::new();
        manager.create_session(session_id).await.unwrap();

        manager
            .persist_event(
                session_id,
                &SessionEvent::Error {
                    message: "event 1".to_string(),
                },
            )
            .await
            .unwrap();

        manager.evict_lru();
        assert!(!manager.is_loaded(session_id));

        let session = manager.get_session(session_id).await.unwrap();
        assert_eq!(session.state.event_sequence, 1);
    }
}
