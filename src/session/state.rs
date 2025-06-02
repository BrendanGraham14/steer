use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::api::messages::{ContentBlock, MessageContent, MessageRole};
use crate::api::{Message, Model, ToolCall};

/// Complete session representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub config: SessionConfig,
    pub state: SessionState,
}

impl Session {
    pub fn new(id: String, config: SessionConfig) -> Self {
        let now = Utc::now();
        Self {
            id,
            created_at: now,
            updated_at: now,
            config,
            state: SessionState::default(),
        }
    }

    pub fn update_timestamp(&mut self) {
        self.updated_at = Utc::now();
    }

    /// Check if session has any recent activity
    pub fn is_recently_active(&self, threshold: chrono::Duration) -> bool {
        let cutoff = Utc::now() - threshold;
        self.updated_at > cutoff
    }
}

/// Session configuration - immutable once created
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub tool_policy: ToolApprovalPolicy,
    pub tool_config: SessionToolConfig,
    pub metadata: HashMap<String, String>,
}

/// Tool approval policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolApprovalPolicy {
    /// Always ask for approval before executing any tool
    AlwaysAsk,

    /// Pre-approved tools execute without asking
    PreApproved(HashSet<String>),

    /// Mixed policy: some tools pre-approved, others require approval
    Mixed {
        pre_approved: HashSet<String>,
        ask_for_others: bool,
    },
}

impl ToolApprovalPolicy {
    pub fn is_tool_approved(&self, tool_name: &str) -> bool {
        match self {
            ToolApprovalPolicy::AlwaysAsk => false,
            ToolApprovalPolicy::PreApproved(tools) => tools.contains(tool_name),
            ToolApprovalPolicy::Mixed {
                pre_approved,
                ask_for_others: _,
            } => pre_approved.contains(tool_name),
        }
    }

    pub fn should_ask_for_approval(&self, tool_name: &str) -> bool {
        match self {
            ToolApprovalPolicy::AlwaysAsk => true,
            ToolApprovalPolicy::PreApproved(tools) => !tools.contains(tool_name),
            ToolApprovalPolicy::Mixed {
                pre_approved,
                ask_for_others,
            } => !pre_approved.contains(tool_name) && *ask_for_others,
        }
    }
}

/// Tool configuration for the session
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionToolConfig {
    /// Disabled tools for this session
    pub disabled_tools: HashSet<String>,

    /// Additional metadata for tool configuration
    pub metadata: HashMap<String, String>,
}

impl SessionToolConfig {
    pub fn is_tool_enabled(&self, tool_name: &str) -> bool {
        !self.disabled_tools.contains(tool_name)
    }

    pub fn disable_tool(&mut self, tool_name: String) {
        self.disabled_tools.insert(tool_name);
    }

    pub fn enable_tool(&mut self, tool_name: &str) {
        self.disabled_tools.remove(tool_name);
    }
}

/// Mutable session state that changes during execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Conversation messages
    pub messages: Vec<Message>,

    /// Tool call tracking
    pub tool_calls: HashMap<String, ToolCallState>,

    /// Tools that have been approved for this session
    pub approved_tools: HashSet<String>,

    /// Last processed event sequence number for replay
    pub last_event_sequence: u64,

    /// Additional runtime metadata
    pub metadata: HashMap<String, String>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            tool_calls: HashMap::new(),
            approved_tools: HashSet::new(),
            last_event_sequence: 0,
            metadata: HashMap::new(),
        }
    }
}

impl SessionState {
    /// Add a message to the conversation
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Get the number of messages in the conversation
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Get the last message in the conversation
    pub fn last_message(&self) -> Option<&Message> {
        self.messages.last()
    }

    /// Add a tool call to tracking
    pub fn add_tool_call(&mut self, tool_call: ToolCall) {
        let state = ToolCallState {
            tool_call: tool_call.clone(),
            status: ToolCallStatus::PendingApproval,
            started_at: None,
            completed_at: None,
            result: None,
        };
        self.tool_calls.insert(tool_call.id, state);
    }

    /// Update tool call status
    pub fn update_tool_call_status(
        &mut self,
        tool_call_id: &str,
        status: ToolCallStatus,
    ) -> Result<(), String> {
        let tool_call = self
            .tool_calls
            .get_mut(tool_call_id)
            .ok_or_else(|| format!("Tool call not found: {}", tool_call_id))?;

        // Update timestamps based on status changes
        match (&tool_call.status, &status) {
            (_, ToolCallStatus::Executing) => {
                tool_call.started_at = Some(Utc::now());
            }
            (_, ToolCallStatus::Completed) | (_, ToolCallStatus::Failed { .. }) => {
                tool_call.completed_at = Some(Utc::now());
            }
            _ => {}
        }

        tool_call.status = status;
        Ok(())
    }

    /// Approve a tool for future use
    pub fn approve_tool(&mut self, tool_name: String) {
        self.approved_tools.insert(tool_name);
    }

    /// Check if a tool is approved
    pub fn is_tool_approved(&self, tool_name: &str) -> bool {
        self.approved_tools.contains(tool_name)
    }

    /// Validate internal consistency
    pub fn validate(&self) -> Result<(), String> {
        // Check that all tool calls referenced in messages exist
        for message in &self.messages {
            let tool_calls = self.extract_tool_calls_from_message(message);
            if !tool_calls.is_empty() {
                for tool_call_id in tool_calls {
                    if !self.tool_calls.contains_key(&tool_call_id) {
                        return Err(format!(
                            "Message references unknown tool call: {}",
                            tool_call_id
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Extract tool call IDs from a message
    fn extract_tool_calls_from_message(&self, message: &Message) -> Vec<String> {
        let mut tool_call_ids = Vec::new();

        match &message.content {
            MessageContent::StructuredContent { content } => {
                for block in &content.0 {
                    match block {
                        ContentBlock::ToolUse { id, .. } => {
                            tool_call_ids.push(id.clone());
                        }
                        ContentBlock::ToolResult { tool_use_id, .. } => {
                            tool_call_ids.push(tool_use_id.clone());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        tool_call_ids
    }

    /// Apply an event to the session state
    pub fn apply_event(&mut self, event: &crate::events::StreamEvent) -> Result<(), String> {
        use crate::events::StreamEvent;

        match event {
            StreamEvent::MessageComplete { message, .. } => {
                self.add_message(message.clone());
            }
            StreamEvent::ToolCallStarted { tool_call, .. } => {
                self.add_tool_call(tool_call.clone());
            }
            StreamEvent::ToolCallCompleted {
                tool_call_id,
                result,
                ..
            } => {
                self.update_tool_call_status(tool_call_id, ToolCallStatus::Completed)?;
                if let Some(tool_call_state) = self.tool_calls.get_mut(tool_call_id) {
                    tool_call_state.result = Some(result.clone());
                }
            }
            StreamEvent::ToolCallFailed {
                tool_call_id,
                error,
                ..
            } => {
                self.update_tool_call_status(
                    tool_call_id,
                    ToolCallStatus::Failed {
                        error: error.clone(),
                    },
                )?;
            }
            StreamEvent::ToolApprovalRequired { tool_call, .. } => {
                // Tool call should already be added with PendingApproval status
                if !self.tool_calls.contains_key(&tool_call.id) {
                    self.add_tool_call(tool_call.clone());
                }
            }
            // Other events don't modify state directly
            _ => {}
        }

        Ok(())
    }
}

/// Tool call state tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallState {
    pub tool_call: ToolCall,
    pub status: ToolCallStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub result: Option<ToolResult>,
}

impl ToolCallState {
    pub fn is_pending(&self) -> bool {
        matches!(self.status, ToolCallStatus::PendingApproval)
    }

    pub fn is_complete(&self) -> bool {
        matches!(
            self.status,
            ToolCallStatus::Completed | ToolCallStatus::Failed { .. }
        )
    }

    pub fn duration(&self) -> Option<chrono::Duration> {
        match (self.started_at, self.completed_at) {
            (Some(start), Some(end)) => Some(end - start),
            _ => None,
        }
    }
}

/// Tool call execution status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolCallStatus {
    PendingApproval,
    Approved,
    Denied,
    Executing,
    Completed,
    Failed { error: String },
}

impl ToolCallStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ToolCallStatus::Completed | ToolCallStatus::Failed { .. } | ToolCallStatus::Denied
        )
    }
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub output: String,
    pub success: bool,
    pub execution_time_ms: u64,
    pub metadata: HashMap<String, String>,
}

impl ToolResult {
    pub fn success(output: String, execution_time_ms: u64) -> Self {
        Self {
            output,
            success: true,
            execution_time_ms,
            metadata: HashMap::new(),
        }
    }

    pub fn failure(error: String, execution_time_ms: u64) -> Self {
        Self {
            output: error,
            success: false,
            execution_time_ms,
            metadata: HashMap::new(),
        }
    }

    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }
}

/// Session metadata for listing and filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The last known model used in this session
    pub last_model: Option<Model>,
    pub message_count: usize,
    pub metadata: HashMap<String, String>,
}

impl From<&Session> for SessionInfo {
    fn from(session: &Session) -> Self {
        Self {
            id: session.id.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            last_model: None, // TODO: Track last model used from events
            message_count: session.state.message_count(),
            metadata: session.config.metadata.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::messages::{MessageContent, MessageRole};

    #[test]
    fn test_session_creation() {
        let config = SessionConfig {
            tool_policy: ToolApprovalPolicy::AlwaysAsk,
            tool_config: SessionToolConfig {
                disabled_tools: HashSet::new(),
                metadata: HashMap::new(),
            },
            metadata: HashMap::new(),
        };
        let session = Session::new("test-session".to_string(), config.clone());

        assert_eq!(session.id, "test-session");
        assert_eq!(
            session
                .config
                .tool_policy
                .should_ask_for_approval("any_tool"),
            true
        );
        assert_eq!(session.state.message_count(), 0);
    }

    #[test]
    fn test_tool_approval_policy() {
        let policy = ToolApprovalPolicy::PreApproved(
            ["read_file", "list_files"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );

        assert!(policy.is_tool_approved("read_file"));
        assert!(!policy.is_tool_approved("write_file"));
        assert!(!policy.should_ask_for_approval("read_file"));
        assert!(policy.should_ask_for_approval("write_file"));
    }

    #[test]
    fn test_session_state_validation() {
        let mut state = SessionState::default();

        // Valid empty state
        assert!(state.validate().is_ok());

        // Add a message
        let message = Message {
            id: Some("msg1".to_string()),
            role: MessageRole::User,
            content: MessageContent::Text {
                content: "Hello".to_string(),
            },
        };
        state.add_message(message);

        assert!(state.validate().is_ok());
        assert_eq!(state.message_count(), 1);
    }

    #[test]
    fn test_tool_call_state_tracking() {
        let mut state = SessionState::default();

        let tool_call = ToolCall {
            id: "tool1".to_string(),
            name: "read_file".to_string(),
            parameters: serde_json::json!({"path": "/test.txt"}),
        };

        state.add_tool_call(tool_call.clone());
        assert!(state.tool_calls.get("tool1").unwrap().is_pending());

        state
            .update_tool_call_status("tool1", ToolCallStatus::Executing)
            .unwrap();
        let tool_state = state.tool_calls.get("tool1").unwrap();
        assert!(tool_state.started_at.is_some());
        assert!(!tool_state.is_complete());

        state
            .update_tool_call_status("tool1", ToolCallStatus::Completed)
            .unwrap();
        let tool_state = state.tool_calls.get("tool1").unwrap();
        assert!(tool_state.completed_at.is_some());
        assert!(tool_state.is_complete());
    }
}
