use crate::app::SystemContext;
use crate::app::conversation::Message;
use crate::app::domain::types::{OpId, RequestId, SessionId};
use crate::config::model::ModelId;
use steer_tools::{ToolCall, ToolSchema};

use super::event::SessionEvent;

#[derive(Debug, Clone)]
pub enum Effect {
    EmitEvent {
        session_id: SessionId,
        event: SessionEvent,
    },

    RequestUserApproval {
        session_id: SessionId,
        request_id: RequestId,
        tool_call: ToolCall,
    },

    ExecuteTool {
        session_id: SessionId,
        op_id: OpId,
        tool_call: ToolCall,
    },

    CallModel {
        session_id: SessionId,
        op_id: OpId,
        model: ModelId,
        messages: Vec<Message>,
        system_context: Option<SystemContext>,
        tools: Vec<ToolSchema>,
    },

    GenerateSessionTitle {
        session_id: SessionId,
        op_id: OpId,
        model: ModelId,
        user_prompt: String,
    },

    ListWorkspaceFiles {
        session_id: SessionId,
    },

    CancelOperation {
        session_id: SessionId,
        op_id: OpId,
    },

    ConnectMcpServer {
        session_id: SessionId,
        config: McpServerConfig,
    },

    DisconnectMcpServer {
        session_id: SessionId,
        server_name: String,
    },

    RequestCompaction {
        session_id: SessionId,
        op_id: OpId,
        model: ModelId,
    },

    ReloadToolSchemas {
        session_id: SessionId,
    },
}

#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub server_name: String,
    pub transport: crate::tools::McpTransport,
    pub tool_filter: crate::session::state::ToolFilter,
}

impl Effect {
    pub fn session_id(&self) -> SessionId {
        match self {
            Effect::EmitEvent { session_id, .. }
            | Effect::RequestUserApproval { session_id, .. }
            | Effect::ExecuteTool { session_id, .. }
            | Effect::CallModel { session_id, .. }
            | Effect::GenerateSessionTitle { session_id, .. }
            | Effect::ListWorkspaceFiles { session_id }
            | Effect::CancelOperation { session_id, .. }
            | Effect::ConnectMcpServer { session_id, .. }
            | Effect::DisconnectMcpServer { session_id, .. }
            | Effect::RequestCompaction { session_id, .. }
            | Effect::ReloadToolSchemas { session_id } => *session_id,
        }
    }

    pub fn is_emit_event(&self) -> bool {
        matches!(self, Effect::EmitEvent { .. })
    }

    pub fn into_event(self) -> Option<SessionEvent> {
        match self {
            Effect::EmitEvent { event, .. } => Some(event),
            _ => None,
        }
    }
}
