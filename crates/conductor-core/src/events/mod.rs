use crate::api::Model;
use crate::app::Message;
use crate::session::SessionInfo;
use chrono::{DateTime, Utc};
use conductor_tools::ToolCall;
use conductor_tools::result::ToolResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Token usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Unified event type for external consumers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    // Message events
    MessagePart {
        content: String,
        message_id: String,
    },
    MessageComplete {
        message: Message,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(default)]
        metadata: HashMap<String, String>,
        model: Model,
    },

    // Tool events
    ToolCallStarted {
        tool_call: ToolCall,
        #[serde(default)]
        metadata: HashMap<String, String>,
        model: Model,
    },
    ToolCallCompleted {
        tool_call_id: String,
        result: ToolResult,
        #[serde(default)]
        metadata: HashMap<String, String>,
        model: Model,
    },
    ToolCallFailed {
        tool_call_id: String,
        error: String,
        #[serde(default)]
        metadata: HashMap<String, String>,
        model: Model,
    },
    ToolApprovalRequired {
        tool_call: ToolCall,
        timeout_ms: Option<u64>,
        #[serde(default)]
        metadata: HashMap<String, String>,
    },

    // Session events
    SessionCreated {
        session_id: String,
        metadata: SessionMetadata,
    },
    SessionResumed {
        session_id: String,
        event_offset: u64,
    },
    SessionSaved {
        session_id: String,
    },

    // Operation events
    OperationStarted {
        operation_id: String,
    },
    OperationCompleted {
        operation_id: String,
    },
    OperationCancelled {
        operation_id: String,
        reason: String,
    },

    // System events
    Error {
        message: String,
        error_type: ErrorType,
    },

    // Workspace events
    WorkspaceChanged,
    WorkspaceFiles {
        files: Vec<String>,
    },
}

/// Event with metadata for persistence and replay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEventWithMetadata {
    pub sequence_num: u64,
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub event: StreamEvent,
}

impl StreamEventWithMetadata {
    pub fn new(sequence_num: u64, session_id: String, event: StreamEvent) -> Self {
        Self {
            sequence_num,
            timestamp: Utc::now(),
            session_id,
            event,
        }
    }
}

/// Session metadata for events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub model: Model,
    pub created_at: DateTime<Utc>,
    pub metadata: HashMap<String, String>,
}

impl From<&SessionInfo> for SessionMetadata {
    fn from(session_info: &SessionInfo) -> Self {
        Self {
            model: session_info
                .last_model
                .unwrap_or(crate::api::Model::ClaudeSonnet4_20250514),
            created_at: session_info.created_at,
            metadata: session_info.metadata.clone(),
        }
    }
}

/// Error types for system events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    /// API error (OpenAI, Anthropic, etc.)
    Api,
    /// Tool execution error
    Tool,
    /// Session management error
    Session,
    /// Persistence/storage error
    Storage,
    /// Authentication/authorization error
    Auth,
    /// Network/transport error
    Network,
    /// Internal server error
    Internal,
    /// Validation error
    Validation,
    /// Resource limit exceeded
    ResourceLimit,
    /// Operation timeout
    Timeout,
}

impl StreamEvent {
    /// Check if this event indicates an error condition
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            StreamEvent::Error { .. } | StreamEvent::ToolCallFailed { .. }
        )
    }

    /// Get the operation ID if this event relates to an operation
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            StreamEvent::OperationStarted { operation_id }
            | StreamEvent::OperationCompleted { operation_id }
            | StreamEvent::OperationCancelled { operation_id, .. } => Some(operation_id),
            _ => None,
        }
    }

    /// Get the session ID if this event relates to a session
    pub fn session_id(&self) -> Option<&str> {
        match self {
            StreamEvent::SessionCreated { session_id, .. }
            | StreamEvent::SessionResumed { session_id, .. }
            | StreamEvent::SessionSaved { session_id } => Some(session_id),
            _ => None,
        }
    }

    /// Get the tool call ID if this event relates to a tool call
    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            StreamEvent::ToolCallStarted { tool_call, .. } => Some(&tool_call.id),
            StreamEvent::ToolCallCompleted { tool_call_id, .. } => Some(tool_call_id),
            StreamEvent::ToolCallFailed { tool_call_id, .. } => Some(tool_call_id),
            StreamEvent::ToolApprovalRequired { tool_call, .. } => Some(&tool_call.id),
            _ => None,
        }
    }

    /// Get the message ID if this event relates to a message
    pub fn message_id(&self) -> Option<&str> {
        match self {
            StreamEvent::MessagePart { message_id, .. } => Some(message_id),
            StreamEvent::MessageComplete { message, .. } => Some(message.id()),
            _ => None,
        }
    }
}

/// Event filter for client subscriptions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFilter {
    /// Only events matching these types
    pub event_types: Option<Vec<String>>,
    /// Only events after this sequence number
    pub after_sequence: Option<u64>,
    /// Only events for these sessions
    pub session_ids: Option<Vec<String>>,
    /// Only events for these operations
    pub operation_ids: Option<Vec<String>>,
    /// Only events for these tool calls
    pub tool_call_ids: Option<Vec<String>>,
}

impl EventFilter {
    /// Create an empty filter (matches all events)
    pub fn all() -> Self {
        Self {
            event_types: None,
            after_sequence: None,
            session_ids: None,
            operation_ids: None,
            tool_call_ids: None,
        }
    }

    /// Create a filter for specific event types
    pub fn for_types(types: Vec<String>) -> Self {
        Self {
            event_types: Some(types),
            after_sequence: None,
            session_ids: None,
            operation_ids: None,
            tool_call_ids: None,
        }
    }

    /// Create a filter for events after a sequence number
    pub fn after_sequence(sequence: u64) -> Self {
        Self {
            event_types: None,
            after_sequence: Some(sequence),
            session_ids: None,
            operation_ids: None,
            tool_call_ids: None,
        }
    }

    /// Create a filter for specific sessions
    pub fn for_sessions(session_ids: Vec<String>) -> Self {
        Self {
            event_types: None,
            after_sequence: None,
            session_ids: Some(session_ids),
            operation_ids: None,
            tool_call_ids: None,
        }
    }

    /// Check if an event matches this filter
    pub fn matches(&self, event_with_metadata: &StreamEventWithMetadata) -> bool {
        // Check sequence number
        if let Some(after_seq) = self.after_sequence {
            if event_with_metadata.sequence_num <= after_seq {
                return false;
            }
        }

        // Check session ID
        if let Some(ref session_ids) = self.session_ids {
            if !session_ids.contains(&event_with_metadata.session_id) {
                return false;
            }
        }

        // Check event type
        if let Some(ref event_types) = self.event_types {
            let event_type = match &event_with_metadata.event {
                StreamEvent::MessagePart { .. } => "message_part",
                StreamEvent::MessageComplete { .. } => "message_complete",
                StreamEvent::ToolCallStarted { .. } => "tool_call_started",
                StreamEvent::ToolCallCompleted { .. } => "tool_call_completed",
                StreamEvent::ToolCallFailed { .. } => "tool_call_failed",
                StreamEvent::ToolApprovalRequired { .. } => "tool_approval_required",
                StreamEvent::SessionCreated { .. } => "session_created",
                StreamEvent::SessionResumed { .. } => "session_resumed",
                StreamEvent::SessionSaved { .. } => "session_saved",
                StreamEvent::OperationStarted { .. } => "operation_started",
                StreamEvent::OperationCompleted { .. } => "operation_completed",
                StreamEvent::OperationCancelled { .. } => "operation_cancelled",
                StreamEvent::Error { .. } => "error",
                StreamEvent::WorkspaceChanged => "workspace_changed",
                StreamEvent::WorkspaceFiles { .. } => "workspace_files",
            };
            if !event_types.contains(&event_type.to_string()) {
                return false;
            }
        }

        // Check operation ID
        if let Some(ref operation_ids) = self.operation_ids {
            if let Some(op_id) = event_with_metadata.event.operation_id() {
                if !operation_ids.contains(&op_id.to_string()) {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Check tool call ID
        if let Some(ref tool_call_ids) = self.tool_call_ids {
            if let Some(tool_id) = event_with_metadata.event.tool_call_id() {
                if !tool_call_ids.contains(&tool_id.to_string()) {
                    return false;
                }
            } else {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Message;
    use crate::app::conversation::AssistantContent;

    #[test]
    fn test_stream_event_serialization() {
        let event = StreamEvent::ToolCallStarted {
            tool_call: ToolCall {
                id: "tool_123".to_string(),
                name: "edit_file".to_string(),
                parameters: serde_json::json!({"path": "/test.txt"}),
            },
            metadata: HashMap::new(),
            model: crate::api::Model::ClaudeSonnet4_20250514,
        };

        let serialized = serde_json::to_string(&event).unwrap();
        let deserialized: StreamEvent = serde_json::from_str(&serialized).unwrap();

        assert!(matches!(deserialized, StreamEvent::ToolCallStarted { .. }));
        match deserialized {
            StreamEvent::ToolCallStarted { tool_call, .. } => {
                assert_eq!(tool_call.name, "edit_file");
                assert_eq!(tool_call.id, "tool_123");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_event_with_metadata() {
        let event = StreamEvent::MessagePart {
            message_id: "msg_123".to_string(),
            content: "Hello".to_string(),
        };
        let event_with_metadata = StreamEventWithMetadata::new(1, "session_123".to_string(), event);

        assert_eq!(event_with_metadata.sequence_num, 1);
        assert_eq!(event_with_metadata.session_id, "session_123");
        assert!(event_with_metadata.timestamp <= Utc::now());
    }

    #[test]
    fn test_event_type_checks() {
        let error_event = StreamEvent::Error {
            message: "Test error".to_string(),
            error_type: ErrorType::Api,
        };
        assert!(error_event.is_error());

        let tool_failed = StreamEvent::ToolCallFailed {
            tool_call_id: "tool_123".to_string(),
            error: "Command failed".to_string(),
            metadata: HashMap::new(),
            model: crate::api::Model::ClaudeSonnet4_20250514,
        };
        assert!(tool_failed.is_error());

        let tool_started = StreamEvent::ToolCallStarted {
            tool_call: ToolCall {
                id: "tool_123".to_string(),
                name: "edit_file".to_string(),
                parameters: serde_json::json!({}),
            },
            metadata: HashMap::new(),
            model: crate::api::Model::ClaudeSonnet4_20250514,
        };
        assert!(!tool_started.is_error());
    }

    #[test]
    fn test_event_id_extraction() {
        let tool_event = StreamEvent::ToolCallStarted {
            tool_call: ToolCall {
                id: "tool_123".to_string(),
                name: "edit_file".to_string(),
                parameters: serde_json::json!({}),
            },
            metadata: HashMap::new(),
            model: crate::api::Model::ClaudeSonnet4_20250514,
        };
        assert_eq!(tool_event.tool_call_id(), Some("tool_123"));

        let message_event = StreamEvent::MessagePart {
            message_id: "msg_123".to_string(),
            content: "Hello".to_string(),
        };
        assert_eq!(message_event.message_id(), Some("msg_123"));

        let operation_event = StreamEvent::OperationStarted {
            operation_id: "op_123".to_string(),
        };
        assert_eq!(operation_event.operation_id(), Some("op_123"));

        let session_event = StreamEvent::SessionCreated {
            session_id: "session_123".to_string(),
            metadata: SessionMetadata {
                model: crate::api::Model::ClaudeSonnet4_20250514,
                created_at: Utc::now(),
                metadata: HashMap::new(),
            },
        };
        assert_eq!(session_event.session_id(), Some("session_123"));
    }

    #[test]
    fn test_event_filter() {
        let event = StreamEvent::ToolCallStarted {
            tool_call: ToolCall {
                id: "tool_123".to_string(),
                name: "edit_file".to_string(),
                parameters: serde_json::json!({}),
            },
            metadata: HashMap::new(),
            model: crate::api::Model::ClaudeSonnet4_20250514,
        };
        let event_with_metadata = StreamEventWithMetadata::new(5, "session_123".to_string(), event);

        // Test sequence filter
        let after_filter = EventFilter::after_sequence(3);
        assert!(after_filter.matches(&event_with_metadata));

        let before_filter = EventFilter::after_sequence(5);
        assert!(!before_filter.matches(&event_with_metadata));

        // Test session filter
        let session_filter = EventFilter::for_sessions(vec!["session_123".to_string()]);
        assert!(session_filter.matches(&event_with_metadata));

        let wrong_session_filter = EventFilter::for_sessions(vec!["session_456".to_string()]);
        assert!(!wrong_session_filter.matches(&event_with_metadata));

        // Test type filter
        let type_filter = EventFilter::for_types(vec!["tool_call_started".to_string()]);
        assert!(type_filter.matches(&event_with_metadata));

        let wrong_type_filter = EventFilter::for_types(vec!["message_part".to_string()]);
        assert!(!wrong_type_filter.matches(&event_with_metadata));
    }

    #[test]
    fn test_message_complete_event() {
        let message = Message::Assistant {
            content: vec![AssistantContent::Text {
                text: "Hello world".to_string(),
            }],
            timestamp: 0,
            id: "msg_123".to_string(),
            thread_id: uuid::Uuid::now_v7(),
            parent_message_id: None,
        };

        let event = StreamEvent::MessageComplete {
            message,
            usage: None,
            metadata: HashMap::new(),
            model: crate::api::Model::ClaudeSonnet4_20250514,
        };
        assert_eq!(event.message_id(), Some("msg_123"));
    }
}
