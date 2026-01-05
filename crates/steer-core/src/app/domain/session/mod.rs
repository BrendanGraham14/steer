pub mod catalog;
pub mod event_store;
pub mod manager;
pub mod sqlite_event_store;

pub use catalog::{SessionCatalog, SessionCatalogError, SessionFilter, SessionSummary};
pub use event_store::{EventStore, EventStoreError, InMemoryEventStore};
pub use manager::{ActiveSession, SessionManager, SessionManagerConfig, SessionManagerError};
pub use sqlite_event_store::SqliteEventStore;
