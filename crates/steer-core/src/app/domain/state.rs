use crate::app::conversation::MessageGraph;
use crate::app::domain::action::McpServerState;
use crate::app::domain::types::{MessageId, OpId, RequestId, SessionId, ToolCallId};
use crate::app::SystemContext;
use crate::config::model::ModelId;
use crate::session::state::SessionConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use steer_tools::{ToolCall, ToolSchema};

#[derive(Debug, Clone)]
pub struct AppState {
    pub session_id: SessionId,
    pub session_config: Option<SessionConfig>,
    pub primary_agent_id: Option<String>,

    pub message_graph: MessageGraph,

    pub cached_system_context: Option<SystemContext>,

    pub tools: Vec<ToolSchema>,

    pub approved_tools: HashSet<String>,
    pub approved_bash_patterns: HashSet<String>,
    pub static_bash_patterns: Vec<String>,
    pub pending_approval: Option<PendingApproval>,
    pub approval_queue: VecDeque<QueuedApproval>,

    pub current_operation: Option<OperationState>,

    pub active_streams: HashMap<OpId, StreamingMessage>,

    pub workspace_files: Vec<String>,

    pub mcp_servers: HashMap<String, McpServerState>,

    pub cancelled_ops: HashSet<OpId>,

    pub operation_models: HashMap<OpId, ModelId>,
    pub operation_messages: HashMap<OpId, MessageId>,

    pub event_sequence: u64,
}

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub request_id: RequestId,
    pub tool_call: ToolCall,
}

#[derive(Debug, Clone)]
pub struct QueuedApproval {
    pub tool_call: ToolCall,
}

#[derive(Debug, Clone)]
pub struct OperationState {
    pub op_id: OpId,
    pub kind: OperationKind,
    pub pending_tool_calls: HashSet<ToolCallId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationKind {
    AgentLoop,
    Compact,
    DirectBash { command: String },
}

#[derive(Debug, Clone)]
pub struct StreamingMessage {
    pub message_id: MessageId,
    pub op_id: OpId,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub byte_count: usize,
}

pub struct StreamingConfig {
    pub max_buffer_bytes: usize,
    pub max_concurrent_streams: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            max_buffer_bytes: 64 * 1024,
            max_concurrent_streams: 3,
        }
    }
}

const MAX_CANCELLED_OPS: usize = 100;

impl AppState {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            session_config: None,
            primary_agent_id: None,
            message_graph: MessageGraph::new(),
            cached_system_context: None,
            tools: Vec::new(),
            approved_tools: HashSet::new(),
            approved_bash_patterns: HashSet::new(),
            static_bash_patterns: Vec::new(),
            pending_approval: None,
            approval_queue: VecDeque::new(),
            current_operation: None,
            active_streams: HashMap::new(),
            workspace_files: Vec::new(),
            mcp_servers: HashMap::new(),
            cancelled_ops: HashSet::new(),
            operation_models: HashMap::new(),
            operation_messages: HashMap::new(),
            event_sequence: 0,
        }
    }

    pub fn with_approved_patterns(mut self, patterns: Vec<String>) -> Self {
        self.static_bash_patterns = patterns;
        self
    }

    pub fn with_approved_tools(mut self, tools: HashSet<String>) -> Self {
        self.approved_tools = tools;
        self
    }

    pub fn is_tool_pre_approved(&self, tool_name: &str) -> bool {
        self.approved_tools.contains(tool_name)
    }

    pub fn is_bash_pattern_approved(&self, command: &str) -> bool {
        for pattern in self
            .static_bash_patterns
            .iter()
            .chain(self.approved_bash_patterns.iter())
        {
            if pattern == command {
                return true;
            }
            if let Ok(glob) = glob::Pattern::new(pattern)
                && glob.matches(command)
            {
                return true;
            }
        }
        false
    }

    pub fn approve_tool(&mut self, tool_name: String) {
        self.approved_tools.insert(tool_name);
    }

    pub fn approve_bash_pattern(&mut self, pattern: String) {
        self.approved_bash_patterns.insert(pattern);
    }

    pub fn record_cancelled_op(&mut self, op_id: OpId) {
        self.cancelled_ops.insert(op_id);
        if self.cancelled_ops.len() > MAX_CANCELLED_OPS
            && let Some(&oldest) = self.cancelled_ops.iter().next()
        {
            self.cancelled_ops.remove(&oldest);
        }
    }

    pub fn is_op_cancelled(&self, op_id: &OpId) -> bool {
        self.cancelled_ops.contains(op_id)
    }

    pub fn has_pending_approval(&self) -> bool {
        self.pending_approval.is_some()
    }

    pub fn start_operation(&mut self, op_id: OpId, kind: OperationKind) {
        self.current_operation = Some(OperationState {
            op_id,
            kind,
            pending_tool_calls: HashSet::new(),
        });
    }

    pub fn complete_operation(&mut self, op_id: OpId) {
        self.operation_models.remove(&op_id);
        self.operation_messages.remove(&op_id);
        if self
            .current_operation
            .as_ref()
            .is_some_and(|op| op.op_id == op_id)
        {
            self.current_operation = None;
        }
    }

    pub fn add_pending_tool_call(&mut self, tool_call_id: ToolCallId) {
        if let Some(ref mut op) = self.current_operation {
            op.pending_tool_calls.insert(tool_call_id);
        }
    }

    pub fn remove_pending_tool_call(&mut self, tool_call_id: &ToolCallId) {
        if let Some(ref mut op) = self.current_operation {
            op.pending_tool_calls.remove(tool_call_id);
        }
    }

    pub fn increment_sequence(&mut self) -> u64 {
        self.event_sequence += 1;
        self.event_sequence
    }
}
