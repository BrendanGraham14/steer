//! Stable client-facing types. Import from here, not from `steer_core` or `steer_tools`.

pub use steer_core::app::conversation::{
    AssistantContent, Message, MessageData, ThoughtContent, UserContent,
};

pub use steer_core::config::model::ModelId;

pub use steer_core::app::domain::delta::ToolCallDelta;
pub use steer_core::app::domain::event::CompactResult;

pub use steer_tools::result::{
    BashResult, EditResult, ExternalResult, FileContentResult, FileListResult, GlobResult,
    GrepResult, ReplaceResult, SearchMatch, SearchResult, TodoListResult, ToolResult,
};
pub use steer_tools::{ToolCall, ToolError};

pub use steer_core::app::domain::types::{
    CompactionRecord, MessageId, OpId, RequestId, ToolCallId,
};

pub use steer_core::session::state::{SessionConfig, SessionToolConfig};

pub use steer_core::session::McpServerInfo;
