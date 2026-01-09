use crate::config::model::ModelId;
use crate::error::Result;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::app::{Message, MessageData};
use crate::tools::{BackendRegistry, McpTransport, ToolBackend};
use steer_tools::tools::read_only_workspace_tools;
use steer_tools::{ToolCall, result::ToolResult};

/// State of an MCP server connection
#[derive(Debug, Clone)]
pub enum McpConnectionState {
    /// Currently attempting to connect
    Connecting,
    /// Successfully connected
    Connected {
        /// Names of tools available from this server
        tool_names: Vec<String>,
    },
    /// Gracefully disconnected
    Disconnected {
        /// Optional reason for disconnection
        reason: Option<String>,
    },
    /// Failed to connect
    Failed {
        /// Error message describing the failure
        error: String,
    },
}

/// Information about an MCP server
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    /// The configured server name
    pub server_name: String,
    /// The transport configuration
    pub transport: McpTransport,
    /// Current connection state
    pub state: McpConnectionState,
    /// Timestamp when this state was last updated
    pub last_updated: DateTime<Utc>,
}

/// Defines the primary execution environment for a session's workspace
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceConfig {
    Local {
        path: PathBuf,
    },
    Remote {
        agent_address: String,
        auth: Option<RemoteAuth>,
    },
}

impl WorkspaceConfig {
    pub fn get_path(&self) -> Option<String> {
        match self {
            WorkspaceConfig::Local { path } => Some(path.to_string_lossy().to_string()),
            WorkspaceConfig::Remote { agent_address, .. } => Some(agent_address.clone()),
        }
    }

    /// Convert to steer_workspace::WorkspaceConfig
    pub fn to_workspace_config(&self) -> steer_workspace::WorkspaceConfig {
        match self {
            WorkspaceConfig::Local { path } => {
                steer_workspace::WorkspaceConfig::Local { path: path.clone() }
            }
            WorkspaceConfig::Remote {
                agent_address,
                auth,
            } => steer_workspace::WorkspaceConfig::Remote {
                address: agent_address.clone(),
                auth: auth.as_ref().map(|a| a.to_workspace_auth()),
            },
        }
    }
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self::Local {
            path: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

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

    /// Build a workspace from this session's configuration
    pub async fn build_workspace(&self) -> Result<Arc<dyn crate::workspace::Workspace>> {
        crate::workspace::create_workspace(&self.config.workspace.to_workspace_config()).await
    }
}

/// Session configuration - immutable once created
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionConfig {
    pub workspace: WorkspaceConfig,
    pub tool_config: SessionToolConfig,
    /// Optional custom system prompt to use for the session. If `None`, Steer will
    /// fall back to its built-in default prompt.
    pub system_prompt: Option<String>,
    pub metadata: HashMap<String, String>,
    pub default_model: ModelId,
}

impl SessionConfig {
    /// Build a BackendRegistry from MCP server configurations.
    /// Returns the registry and a map of MCP server connection states.
    pub async fn build_registry(
        &self,
    ) -> Result<(BackendRegistry, HashMap<String, McpServerInfo>)> {
        let mut registry = BackendRegistry::new();
        let mut mcp_servers = HashMap::new();

        for backend_config in &self.tool_config.backends {
            let BackendConfig::Mcp {
                server_name,
                transport,
                tool_filter,
            } = backend_config;

            tracing::info!(
                "Attempting to initialize MCP backend '{}' with transport: {:?}",
                server_name,
                transport
            );

            let mut server_info = McpServerInfo {
                server_name: server_name.clone(),
                transport: transport.clone(),
                state: McpConnectionState::Connecting,
                last_updated: Utc::now(),
            };

            match crate::tools::McpBackend::new(
                server_name.clone(),
                transport.clone(),
                tool_filter.clone(),
            )
            .await
            {
                Ok(mcp_backend) => {
                    let tool_names = mcp_backend.supported_tools().await;
                    let tool_count = tool_names.len();
                    tracing::info!(
                        "Successfully initialized MCP backend '{}' with {} tools",
                        server_name,
                        tool_count
                    );
                    server_info.state = McpConnectionState::Connected { tool_names };
                    server_info.last_updated = Utc::now();
                    registry
                        .register(format!("mcp_{server_name}"), Arc::new(mcp_backend))
                        .await;
                }
                Err(e) => {
                    tracing::error!("Failed to initialize MCP backend '{}': {}", server_name, e);
                    server_info.state = McpConnectionState::Failed {
                        error: e.to_string(),
                    };
                    server_info.last_updated = Utc::now();
                }
            }

            mcp_servers.insert(server_name.clone(), server_info);
        }

        Ok((registry, mcp_servers))
    }

    /// Filter tools based on visibility settings
    pub fn filter_tools_by_visibility(
        &self,
        tools: Vec<steer_tools::ToolSchema>,
    ) -> Vec<steer_tools::ToolSchema> {
        match &self.tool_config.visibility {
            ToolVisibility::All => tools,
            ToolVisibility::ReadOnly => {
                let read_only_names: HashSet<String> = read_only_workspace_tools()
                    .iter()
                    .map(|t| t.name().to_string())
                    .collect();

                tools
                    .into_iter()
                    .filter(|schema| read_only_names.contains(&schema.name))
                    .collect()
            }
            ToolVisibility::Whitelist(allowed) => tools
                .into_iter()
                .filter(|schema| allowed.contains(&schema.name))
                .collect(),
            ToolVisibility::Blacklist(blocked) => tools
                .into_iter()
                .filter(|schema| !blocked.contains(&schema.name))
                .collect(),
        }
    }

    /// Minimal read-only configuration
    pub fn read_only(default_model: ModelId) -> Self {
        Self {
            workspace: WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            },
            tool_config: SessionToolConfig::read_only(),
            system_prompt: None,
            metadata: HashMap::new(),
            default_model,
        }
    }
}

/// Tool visibility configuration - controls which tools are shown to the AI agent
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolVisibility {
    /// Show all registered tools to the AI
    All,

    /// Only show read-only tools to the AI
    ReadOnly,

    /// Show only specific tools to the AI (whitelist)
    Whitelist(HashSet<String>),

    /// Hide specific tools from the AI (blacklist)
    Blacklist(HashSet<String>),
}

impl Default for ToolVisibility {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum UnapprovedBehavior {
    #[default]
    Prompt,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolRule {
    Bash { patterns: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct ApprovalRules {
    #[serde(default)]
    pub tools: HashSet<String>,
    #[serde(default)]
    pub per_tool: HashMap<String, ToolRule>,
}

impl ApprovalRules {
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.per_tool.is_empty()
    }

    pub fn bash_patterns(&self) -> Option<&[String]> {
        self.per_tool.get("bash").map(|rule| match rule {
            ToolRule::Bash { patterns } => patterns.as_slice(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolApprovalPolicy {
    pub default_behavior: UnapprovedBehavior,
    #[serde(default)]
    pub preapproved: ApprovalRules,
}

impl Default for ToolApprovalPolicy {
    fn default() -> Self {
        Self {
            default_behavior: UnapprovedBehavior::Prompt,
            preapproved: ApprovalRules::default(),
        }
    }
}

impl ToolApprovalPolicy {
    pub fn tool_decision(&self, tool_name: &str) -> ToolDecision {
        if self.preapproved.tools.contains(tool_name) {
            ToolDecision::Allow
        } else {
            match self.default_behavior {
                UnapprovedBehavior::Prompt => ToolDecision::Ask,
                UnapprovedBehavior::Deny => ToolDecision::Deny,
            }
        }
    }

    pub fn is_bash_pattern_preapproved(&self, command: &str) -> bool {
        let Some(patterns) = self.preapproved.bash_patterns() else {
            return false;
        };
        patterns.iter().any(|pattern| {
            if pattern == command {
                return true;
            }
            glob::Pattern::new(pattern)
                .map(|glob| glob.matches(command))
                .unwrap_or(false)
        })
    }

    pub fn pre_approved_tools(&self) -> &HashSet<String> {
        &self.preapproved.tools
    }
}

/// Authentication configuration for remote backends
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum RemoteAuth {
    Bearer { token: String },
    ApiKey { key: String },
}

impl RemoteAuth {
    /// Convert to steer_workspace RemoteAuth type
    pub fn to_workspace_auth(&self) -> steer_workspace::RemoteAuth {
        match self {
            RemoteAuth::Bearer { token } => steer_workspace::RemoteAuth::BearerToken(token.clone()),
            RemoteAuth::ApiKey { key } => steer_workspace::RemoteAuth::ApiKey(key.clone()),
        }
    }
}

/// Tool filtering configuration for backends
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolFilter {
    /// Include all available tools
    All,
    /// Include only the specified tools
    Include(Vec<String>),
    /// Include all tools except the specified ones
    Exclude(Vec<String>),
}

impl Default for ToolFilter {
    fn default() -> Self {
        Self::All
    }
}

/// Configuration for MCP server backends
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackendConfig {
    Mcp {
        server_name: String,
        transport: McpTransport,
        tool_filter: ToolFilter,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionToolConfig {
    pub backends: Vec<BackendConfig>,
    pub visibility: ToolVisibility,
    pub approval_policy: ToolApprovalPolicy,
    pub metadata: HashMap<String, String>,
}

impl Default for SessionToolConfig {
    fn default() -> Self {
        Self {
            backends: Vec::new(),
            visibility: ToolVisibility::All,
            approval_policy: ToolApprovalPolicy::default(),
            metadata: HashMap::new(),
        }
    }
}

impl SessionToolConfig {
    pub fn read_only() -> Self {
        Self {
            backends: Vec::new(),
            visibility: ToolVisibility::ReadOnly,
            approval_policy: ToolApprovalPolicy::default(),
            metadata: HashMap::new(),
        }
    }
}

/// Mutable session state that changes during execution
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    /// Conversation messages
    pub messages: Vec<Message>,

    /// Tool call tracking
    pub tool_calls: HashMap<String, ToolCallState>,

    /// Tools that have been approved for this session
    pub approved_tools: HashSet<String>,

    /// Bash commands that have been approved for this session (dynamically added)
    #[serde(default)]
    pub approved_bash_patterns: HashSet<String>,

    /// Last processed event sequence number for replay
    pub last_event_sequence: u64,

    /// Additional runtime metadata
    pub metadata: HashMap<String, String>,

    /// The ID of the currently active message (head of selected branch)
    /// None means use last message semantics for backward compatibility
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_message_id: Option<String>,

    /// Status of MCP server connections
    /// This is a transient field that is rebuilt on session activation
    #[serde(default, skip_serializing, skip_deserializing)]
    pub mcp_servers: HashMap<String, McpServerInfo>,
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
    ) -> std::result::Result<(), String> {
        let tool_call = self
            .tool_calls
            .get_mut(tool_call_id)
            .ok_or_else(|| format!("Tool call not found: {tool_call_id}"))?;

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
    pub fn validate(&self) -> std::result::Result<(), String> {
        // Check that all tool calls referenced in messages exist
        for message in &self.messages {
            let tool_calls = self.extract_tool_calls_from_message(message);
            if !tool_calls.is_empty() {
                for tool_call_id in tool_calls {
                    if !self.tool_calls.contains_key(&tool_call_id) {
                        return Err(format!(
                            "Message references unknown tool call: {tool_call_id}"
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

        match &message.data {
            MessageData::Assistant { content, .. } => {
                for c in content {
                    if let crate::app::conversation::AssistantContent::ToolCall { tool_call } = c {
                        tool_call_ids.push(tool_call.id.clone());
                    }
                }
            }
            MessageData::Tool { tool_use_id, .. } => {
                tool_call_ids.push(tool_use_id.clone());
            }
            _ => {}
        }

        tool_call_ids
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

/// Tool execution statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionStats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>, // Legacy string output
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_output: Option<serde_json::Value>, // Typed JSON output
    pub result_type: Option<String>, // Type name (e.g., "SearchResult")
    pub success: bool,
    pub execution_time_ms: u64,
    pub metadata: HashMap<String, String>,
}

impl ToolExecutionStats {
    pub fn success(output: String, execution_time_ms: u64) -> Self {
        Self {
            output: Some(output),
            json_output: None,
            result_type: None,
            success: true,
            execution_time_ms,
            metadata: HashMap::new(),
        }
    }

    pub fn success_typed(
        json_output: serde_json::Value,
        result_type: String,
        execution_time_ms: u64,
    ) -> Self {
        Self {
            output: None,
            json_output: Some(json_output),
            result_type: Some(result_type),
            success: true,
            execution_time_ms,
            metadata: HashMap::new(),
        }
    }

    pub fn failure(error: String, execution_time_ms: u64) -> Self {
        Self {
            output: Some(error),
            json_output: None,
            result_type: None,
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
    pub last_model: Option<ModelId>,
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
    use crate::app::conversation::{Message, MessageData, UserContent};
    use crate::config::model::builtin::claude_sonnet_4_5 as test_model;
    use steer_tools::tools::{BASH_TOOL_NAME, EDIT_TOOL_NAME};

    #[test]
    fn test_session_creation() {
        let config = SessionConfig {
            workspace: WorkspaceConfig::Local {
                path: PathBuf::from("/test/path"),
            },
            tool_config: SessionToolConfig::default(),
            system_prompt: None,
            metadata: HashMap::new(),
            default_model: test_model(),
        };
        let session = Session::new("test-session".to_string(), config.clone());

        assert_eq!(session.id, "test-session");
        assert_eq!(
            session
                .config
                .tool_config
                .approval_policy
                .tool_decision("any_tool"),
            ToolDecision::Ask
        );
        assert_eq!(session.state.message_count(), 0);
    }

    #[test]
    fn test_tool_approval_policy_prompt_unapproved() {
        let policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Prompt,
            preapproved: ApprovalRules {
                tools: ["read_file", "list_files"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                per_tool: HashMap::new(),
            },
        };

        assert_eq!(policy.tool_decision("read_file"), ToolDecision::Allow);
        assert_eq!(policy.tool_decision("write_file"), ToolDecision::Ask);
    }

    #[test]
    fn test_tool_approval_policy_deny_unapproved() {
        let policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Deny,
            preapproved: ApprovalRules {
                tools: ["read_file", "list_files"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                per_tool: HashMap::new(),
            },
        };

        assert_eq!(policy.tool_decision("read_file"), ToolDecision::Allow);
        assert_eq!(policy.tool_decision("write_file"), ToolDecision::Deny);
    }

    #[test]
    fn test_tool_approval_policy_default() {
        let policy = ToolApprovalPolicy::default();

        assert_eq!(policy.tool_decision("read_file"), ToolDecision::Ask);
        assert_eq!(policy.tool_decision("write_file"), ToolDecision::Ask);
    }

    #[test]
    fn test_bash_pattern_matching() {
        let policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Prompt,
            preapproved: ApprovalRules {
                tools: HashSet::new(),
                per_tool: [(
                    "bash".to_string(),
                    ToolRule::Bash {
                        patterns: vec![
                            "git status".to_string(),
                            "git log*".to_string(),
                            "git * --oneline".to_string(),
                            "ls -?a*".to_string(),
                            "cargo build*".to_string(),
                        ],
                    },
                )]
                .into_iter()
                .collect(),
            },
        };

        assert!(policy.is_bash_pattern_preapproved("git status"));
        assert!(policy.is_bash_pattern_preapproved("git log --oneline"));
        assert!(policy.is_bash_pattern_preapproved("git show --oneline"));
        assert!(policy.is_bash_pattern_preapproved("ls -la"));
        assert!(policy.is_bash_pattern_preapproved("cargo build --release"));
        assert!(!policy.is_bash_pattern_preapproved("git commit"));
        assert!(!policy.is_bash_pattern_preapproved("ls -l"));
        assert!(!policy.is_bash_pattern_preapproved("rm -rf /"));
    }

    #[test]
    fn test_session_state_validation() {
        let mut state = SessionState::default();

        // Valid empty state
        assert!(state.validate().is_ok());

        // Add a message
        let message = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Hello".to_string(),
                }],
            },
            timestamp: 123456789,
            id: "msg1".to_string(),
            parent_message_id: None,
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

    #[test]
    fn test_session_tool_config_default() {
        let config = SessionToolConfig::default();
        assert!(config.backends.is_empty());
    }

    #[test]
    fn test_tool_filter_exclude() {
        let excluded =
            ToolFilter::Exclude(vec![BASH_TOOL_NAME.to_string(), EDIT_TOOL_NAME.to_string()]);

        if let ToolFilter::Exclude(tools) = &excluded {
            assert_eq!(tools.len(), 2);
            assert!(tools.contains(&BASH_TOOL_NAME.to_string()));
            assert!(tools.contains(&EDIT_TOOL_NAME.to_string()));
        } else {
            panic!("Expected ToolFilter::Exclude");
        }
    }

    #[test]
    fn test_session_tool_config_read_only() {
        let config = SessionToolConfig::read_only();
        assert_eq!(config.backends.len(), 0);
        assert!(matches!(config.visibility, ToolVisibility::ReadOnly));
        assert_eq!(
            config.approval_policy.default_behavior,
            UnapprovedBehavior::Prompt
        );
    }

    #[tokio::test]
    async fn test_session_config_build_registry_no_default_backends() {
        // Test that BackendRegistry only contains user-configured backends.
        // Static tools (dispatch_agent, web_fetch) are now in ToolRegistry,
        // not BackendRegistry.
        let config = SessionConfig {
            workspace: WorkspaceConfig::Local {
                path: PathBuf::from("/test/path"),
            },
            tool_config: SessionToolConfig::default(), // No backends configured
            system_prompt: None,
            metadata: HashMap::new(),
            default_model: test_model(),
        };

        let (registry, _mcp_servers) = config.build_registry().await.unwrap();
        let schemas = registry.get_tool_schemas().await;

        assert!(
            schemas.is_empty(),
            "BackendRegistry should be empty with default config; got: {:?}",
            schemas.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    // Test removed: workspace tools are no longer in the registry

    // Test removed: tool visibility filtering for workspace tools happens at the Workspace level

    // Test removed: workspace backend no longer exists in the registry

    #[test]
    fn test_mcp_status_tracking() {
        // Test that MCP server info is properly tracked in session state
        let mut session_state = SessionState::default();

        // Add some MCP server info
        let mcp_info = McpServerInfo {
            server_name: "test-server".to_string(),
            transport: crate::tools::McpTransport::Stdio {
                command: "python".to_string(),
                args: vec!["-m".to_string(), "test_server".to_string()],
            },
            state: McpConnectionState::Connected {
                tool_names: vec![
                    "tool1".to_string(),
                    "tool2".to_string(),
                    "tool3".to_string(),
                    "tool4".to_string(),
                    "tool5".to_string(),
                ],
            },
            last_updated: Utc::now(),
        };

        session_state
            .mcp_servers
            .insert("test-server".to_string(), mcp_info.clone());

        // Verify it's stored
        assert_eq!(session_state.mcp_servers.len(), 1);
        let stored = session_state.mcp_servers.get("test-server").unwrap();
        assert_eq!(stored.server_name, "test-server");
        assert!(matches!(
            stored.state,
            McpConnectionState::Connected { ref tool_names } if tool_names.len() == 5
        ));

        // Test failed connection
        let failed_info = McpServerInfo {
            server_name: "failed-server".to_string(),
            transport: crate::tools::McpTransport::Tcp {
                host: "localhost".to_string(),
                port: 9999,
            },
            state: McpConnectionState::Failed {
                error: "Connection refused".to_string(),
            },
            last_updated: Utc::now(),
        };

        session_state
            .mcp_servers
            .insert("failed-server".to_string(), failed_info);
        assert_eq!(session_state.mcp_servers.len(), 2);
    }

    #[tokio::test]
    async fn test_mcp_server_tracking_in_build_registry() {
        // Create a session config with both good and bad MCP servers
        let mut config = SessionConfig::read_only(test_model());

        // This one should fail (invalid transport)
        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "bad-server".to_string(),
            transport: crate::tools::McpTransport::Tcp {
                host: "nonexistent.invalid".to_string(),
                port: 12345,
            },
            tool_filter: ToolFilter::All,
        });

        // This one would succeed if we had a real server running
        config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "good-server".to_string(),
            transport: crate::tools::McpTransport::Stdio {
                command: "echo".to_string(),
                args: vec!["test".to_string()],
            },
            tool_filter: ToolFilter::All,
        });

        let (_registry, mcp_servers) = config.build_registry().await.unwrap();

        // Should have tracked both servers
        assert_eq!(mcp_servers.len(), 2);

        // Check the bad server
        let bad_server = mcp_servers.get("bad-server").unwrap();
        assert_eq!(bad_server.server_name, "bad-server");
        assert!(matches!(
            bad_server.state,
            McpConnectionState::Failed { .. }
        ));

        // Check the good server (will also fail in tests since echo isn't an MCP server)
        let good_server = mcp_servers.get("good-server").unwrap();
        assert_eq!(good_server.server_name, "good-server");
        assert!(matches!(
            good_server.state,
            McpConnectionState::Failed { .. }
        ));
    }

    #[test]
    fn test_backend_config_mcp_variant() {
        let mcp_config = BackendConfig::Mcp {
            server_name: "test-mcp".to_string(),
            transport: crate::tools::McpTransport::Stdio {
                command: "python".to_string(),
                args: vec!["-m".to_string(), "test_server".to_string()],
            },
            tool_filter: ToolFilter::All,
        };

        let BackendConfig::Mcp {
            server_name,
            transport,
            ..
        } = mcp_config;

        assert_eq!(server_name, "test-mcp");
        if let crate::tools::McpTransport::Stdio { command, args } = transport {
            assert_eq!(command, "python");
            assert_eq!(args.len(), 2);
        } else {
            panic!("Expected Stdio transport");
        }
    }
}
