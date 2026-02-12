//! State modules for the TUI layer.

pub mod chat_store;
pub use chat_store::ChatStore;

pub mod double_tap;
pub use double_tap::DoubleTapTracker;

pub mod file_cache;
pub mod llm_usage;
pub mod setup;
pub mod tool_registry;

pub use file_cache::FileCache;
pub use llm_usage::{LlmUsageSnapshot, LlmUsageState};
pub use setup::{AuthStatus, RemoteProviderConfig, RemoteProviderRegistry, SetupState, SetupStep};
pub use tool_registry::{ToolCallInfo, ToolCallRegistry, ToolRegistryMetrics, ToolStatus};
