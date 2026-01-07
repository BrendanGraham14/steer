use crate::app::domain::types::{MessageId, OpId, ToolCallId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamDelta {
    TextChunk {
        op_id: OpId,
        message_id: MessageId,
        delta: String,
    },

    ThinkingChunk {
        op_id: OpId,
        message_id: MessageId,
        delta: String,
    },

    ToolCallChunk {
        op_id: OpId,
        message_id: MessageId,
        tool_call_id: ToolCallId,
        delta: ToolCallDelta,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolCallDelta {
    Name(String),
    ArgumentChunk(String),
}

impl StreamDelta {
    pub fn op_id(&self) -> OpId {
        match self {
            StreamDelta::TextChunk { op_id, .. }
            | StreamDelta::ThinkingChunk { op_id, .. }
            | StreamDelta::ToolCallChunk { op_id, .. } => *op_id,
        }
    }

    pub fn message_id(&self) -> Option<&MessageId> {
        match self {
            StreamDelta::TextChunk { message_id, .. }
            | StreamDelta::ThinkingChunk { message_id, .. }
            | StreamDelta::ToolCallChunk { message_id, .. } => Some(message_id),
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self, StreamDelta::TextChunk { .. })
    }

    pub fn is_thinking(&self) -> bool {
        matches!(self, StreamDelta::ThinkingChunk { .. })
    }
}
