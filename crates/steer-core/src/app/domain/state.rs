use crate::app::SystemContext;
use crate::app::conversation::MessageGraph;
use crate::app::conversation::UserContent;
use crate::app::domain::action::McpServerState;
use crate::app::domain::types::{MessageId, OpId, RequestId, SessionId, ToolCallId};
use crate::config::model::ModelId;
use crate::prompts::system_prompt_for_model;
use crate::session::state::SessionConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use steer_tools::{ToolCall, ToolSchema};

#[derive(Debug, Clone)]
pub struct AppState {
    pub session_id: SessionId,
    pub session_config: Option<SessionConfig>,
    pub base_session_config: Option<SessionConfig>,
    pub primary_agent_id: Option<String>,

    pub message_graph: MessageGraph,

    pub cached_system_context: Option<SystemContext>,

    pub tools: Vec<ToolSchema>,

    pub approved_tools: HashSet<String>,
    pub approved_bash_patterns: HashSet<String>,
    pub static_bash_patterns: Vec<String>,
    pub pending_approval: Option<PendingApproval>,
    pub approval_queue: VecDeque<QueuedApproval>,
    pub queued_work: VecDeque<QueuedWorkItem>,

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
pub struct QueuedUserMessage {
    pub content: Vec<UserContent>,
    pub op_id: OpId,
    pub message_id: MessageId,
    pub model: ModelId,
    pub queued_at: u64,
}

#[derive(Debug, Clone)]
pub struct QueuedBashCommand {
    pub command: String,
    pub op_id: OpId,
    pub message_id: MessageId,
    pub queued_at: u64,
}

#[derive(Debug, Clone)]
pub enum QueuedWorkItem {
    UserMessage(QueuedUserMessage),
    DirectBash(QueuedBashCommand),
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
            base_session_config: None,
            primary_agent_id: None,
            message_graph: MessageGraph::new(),
            cached_system_context: None,
            tools: Vec::new(),
            approved_tools: HashSet::new(),
            approved_bash_patterns: HashSet::new(),
            static_bash_patterns: Vec::new(),
            pending_approval: None,
            approval_queue: VecDeque::new(),
            queued_work: VecDeque::new(),
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

    pub fn has_active_operation(&self) -> bool {
        self.current_operation.is_some()
    }

    pub fn queue_user_message(&mut self, item: QueuedUserMessage) {
        if let Some(QueuedWorkItem::UserMessage(tail)) = self.queued_work.back_mut() {
            if tail.content.iter().any(|item| {
                !matches!(item, UserContent::Text { text } if text.as_str().trim().is_empty())
            }) {
                tail.content.push(UserContent::Text {
                    text: "\n\n".to_string(),
                });
            }
            tail.content.extend(item.content);
            tail.op_id = item.op_id;
            tail.message_id = item.message_id;
            tail.model = item.model;
            tail.queued_at = item.queued_at;
            return;
        }
        self.queued_work
            .push_back(QueuedWorkItem::UserMessage(item));
    }

    pub fn queue_bash_command(&mut self, item: QueuedBashCommand) {
        self.queued_work.push_back(QueuedWorkItem::DirectBash(item));
    }

    pub fn pop_next_queued_work(&mut self) -> Option<QueuedWorkItem> {
        self.queued_work.pop_front()
    }

    pub fn queued_summary(&self) -> (Option<QueuedWorkItem>, usize) {
        (self.queued_work.front().cloned(), self.queued_work.len())
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

    pub fn apply_session_config(
        &mut self,
        config: &SessionConfig,
        primary_agent_id: Option<String>,
        update_base: bool,
    ) {
        self.session_config = Some(config.clone());
        let prompt = config
            .system_prompt
            .as_ref()
            .and_then(|prompt| {
                if prompt.trim().is_empty() {
                    None
                } else {
                    Some(prompt.clone())
                }
            })
            .unwrap_or_else(|| system_prompt_for_model(&config.default_model));
        let environment = self
            .cached_system_context
            .as_ref()
            .and_then(|context| context.environment.clone());
        self.cached_system_context = Some(SystemContext::with_environment(prompt, environment));

        self.approved_tools
            .clone_from(config.tool_config.approval_policy.pre_approved_tools());
        self.approved_bash_patterns.clear();
        self.static_bash_patterns = config
            .tool_config
            .approval_policy
            .preapproved
            .bash_patterns()
            .map(|patterns| patterns.to_vec())
            .unwrap_or_default();
        self.pending_approval = None;
        self.approval_queue.clear();

        if let Some(primary_agent_id) = primary_agent_id.or_else(|| config.primary_agent_id.clone())
        {
            self.primary_agent_id = Some(primary_agent_id);
        }

        if update_base {
            self.base_session_config = Some(config.clone());
        }
    }
}
