use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::api::Model;
use crate::app::Message;
use crate::tools::{BackendRegistry, LocalBackend, ToolBackend};
use conductor_tools::tools::read_only_workspace_tools;
use conductor_tools::{ToolCall, result::ToolResult};

/// Defines the primary execution environment for a session's workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceConfig {
    Local,
    Remote {
        agent_address: String,
        auth: Option<RemoteAuth>,
    },
    Container {
        image: String,
        runtime: ContainerRuntime,
    },
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self::Local
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
        crate::workspace::create_workspace(&self.config.workspace).await
    }
}

/// Session configuration - immutable once created
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub workspace: WorkspaceConfig,
    pub tool_config: SessionToolConfig,
    /// Optional custom system prompt to use for the session. If `None`, Conductor will
    /// fall back to its built-in default prompt.
    pub system_prompt: Option<String>,
    pub metadata: HashMap<String, String>,
}

impl SessionConfig {
    /// Build a BackendRegistry from this configuration for external tools only.
    /// Workspace tools are now handled directly by the Workspace.
    pub async fn build_registry(&self) -> Result<BackendRegistry> {
        let mut registry = BackendRegistry::new();

        // 1. Register all USER-DEFINED backends first.
        // Their tool mappings may be overwritten by the more authoritative backends below.
        for (idx, backend_config) in self.tool_config.backends.iter().enumerate() {
            match backend_config {
                BackendConfig::Local { tool_filter } => {
                    let backend = match tool_filter {
                        ToolFilter::All => LocalBackend::full(),
                        ToolFilter::Include(tools) => LocalBackend::with_tools(tools.clone()),
                        ToolFilter::Exclude(excluded) => {
                            LocalBackend::without_tools(excluded.clone())
                        }
                    };
                    registry.register(format!("user_local_{}", idx), Arc::new(backend));
                }
                BackendConfig::Remote {
                    name,
                    endpoint,
                    auth: _,
                    tool_filter: _,
                } => {
                    // Remote backends require conductor-grpc and cannot be created in conductor-core
                    tracing::warn!(
                        "Remote backend '{}' at {} requires conductor-grpc. Skipping.",
                        name,
                        endpoint
                    );
                }
                BackendConfig::Container {
                    image,
                    runtime: _,
                    tool_filter: _,
                } => {
                    tracing::warn!(
                        "User-defined Container backend with image '{}' not yet supported, skipping.",
                        image
                    );
                }
                BackendConfig::Mcp {
                    server_name,
                    transport: _,
                    command: _,
                    args: _,
                    tool_filter: _,
                } => {
                    tracing::warn!(
                        "User-defined MCP backend '{}' not yet supported, skipping.",
                        server_name
                    );
                }
            }
        }

        // 2. Register SERVER tools (like dispatch_agent and web_fetch).
        // These are external tools, not workspace tools.
        let server_backend = LocalBackend::server_only();
        if !server_backend.supported_tools().is_empty() {
            registry.register("server".to_string(), Arc::new(server_backend));
        }

        // Note: Workspace tools are no longer registered here.
        // They are handled directly by the Workspace implementation.

        Ok(registry)
    }

    /// Filter tools based on visibility settings
    pub fn filter_tools_by_visibility(
        &self,
        tools: Vec<conductor_tools::ToolSchema>,
    ) -> Vec<conductor_tools::ToolSchema> {
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
    pub fn read_only() -> Self {
        Self {
            workspace: WorkspaceConfig::Local,
            tool_config: SessionToolConfig::read_only(),
            system_prompt: None,
            metadata: HashMap::new(),
        }
    }
}

/// Tool visibility configuration - controls which tools are shown to the AI agent
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Tool approval policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolApprovalPolicy {
    /// Always ask for approval before executing any tool
    AlwaysAsk,

    /// Pre-approved tools execute without asking
    PreApproved { tools: HashSet<String> },

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
            ToolApprovalPolicy::PreApproved { tools } => tools.contains(tool_name),
            ToolApprovalPolicy::Mixed {
                pre_approved,
                ask_for_others: _,
            } => pre_approved.contains(tool_name),
        }
    }

    pub fn should_ask_for_approval(&self, tool_name: &str) -> bool {
        match self {
            ToolApprovalPolicy::AlwaysAsk => true,
            ToolApprovalPolicy::PreApproved { tools } => !tools.contains(tool_name),
            ToolApprovalPolicy::Mixed {
                pre_approved,
                ask_for_others,
            } => !pre_approved.contains(tool_name) && *ask_for_others,
        }
    }
}

/// Authentication configuration for remote backends
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RemoteAuth {
    Bearer { token: String },
    ApiKey { key: String },
}

/// Tool filtering configuration for backends
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// Container runtime configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContainerRuntime {
    Docker,
    Podman,
}

/// Backend configuration for different tool execution environments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackendConfig {
    Local {
        /// Tool filtering configuration for the local backend
        tool_filter: ToolFilter,
    },
    Remote {
        name: String,
        endpoint: String,
        auth: Option<RemoteAuth>,
        tool_filter: ToolFilter,
    },
    Container {
        image: String,
        runtime: ContainerRuntime,
        tool_filter: ToolFilter,
    },
    Mcp {
        server_name: String,
        transport: String,
        command: String,
        args: Vec<String>,
        tool_filter: ToolFilter,
    },
}

/// Tool configuration for the session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionToolConfig {
    /// Backend configurations for this session
    pub backends: Vec<BackendConfig>,
    /// Tool visibility - controls which tools are shown to the AI agent
    pub visibility: ToolVisibility,
    /// Tool approval policy - controls when user approval is needed
    pub approval_policy: ToolApprovalPolicy,
    /// Additional metadata for tool configuration
    pub metadata: HashMap<String, String>,
}

impl Default for SessionToolConfig {
    fn default() -> Self {
        Self {
            backends: Vec::new(),
            visibility: ToolVisibility::All,
            approval_policy: ToolApprovalPolicy::AlwaysAsk,
            metadata: HashMap::new(),
        }
    }
}

impl SessionToolConfig {
    /// Minimal read-only configuration
    pub fn read_only() -> Self {
        Self {
            backends: Vec::new(), // Use default backends
            visibility: ToolVisibility::ReadOnly,
            approval_policy: ToolApprovalPolicy::AlwaysAsk,
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

    /// Last processed event sequence number for replay
    pub last_event_sequence: u64,

    /// Additional runtime metadata
    pub metadata: HashMap<String, String>,
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

        match message {
            Message::Assistant { content, .. } => {
                for c in content {
                    if let crate::app::conversation::AssistantContent::ToolCall { tool_call } = c {
                        tool_call_ids.push(tool_call.id.clone());
                    }
                }
            }
            Message::Tool { tool_use_id, .. } => {
                tool_call_ids.push(tool_use_id.clone());
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
    use crate::app::conversation::{Message, UserContent};
    use conductor_tools::tools::{BASH_TOOL_NAME, EDIT_TOOL_NAME, LS_TOOL_NAME, VIEW_TOOL_NAME};

    #[test]
    fn test_session_creation() {
        let config = SessionConfig {
            workspace: WorkspaceConfig::Local,
            tool_config: SessionToolConfig::default(),
            system_prompt: None,
            metadata: HashMap::new(),
        };
        let session = Session::new("test-session".to_string(), config.clone());

        assert_eq!(session.id, "test-session");
        assert!(
            session
                .config
                .tool_config
                .approval_policy
                .should_ask_for_approval("any_tool")
        );
        assert_eq!(session.state.message_count(), 0);
    }

    #[test]
    fn test_tool_approval_policy() {
        let policy = ToolApprovalPolicy::PreApproved {
            tools: ["read_file", "list_files"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };

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
        let message = Message::User {
            content: vec![UserContent::Text {
                text: "Hello".to_string(),
            }],
            timestamp: 123456789,
            id: "msg1".to_string(),
            thread_id: uuid::Uuid::now_v7(),
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
        // Test that we can exclude specific tools
        let config = SessionToolConfig {
            backends: vec![BackendConfig::Local {
                tool_filter: ToolFilter::Exclude(vec![
                    BASH_TOOL_NAME.to_string(),
                    EDIT_TOOL_NAME.to_string(),
                ]),
            }],
            visibility: ToolVisibility::All,
            approval_policy: ToolApprovalPolicy::AlwaysAsk,
            metadata: HashMap::new(),
        };

        match &config.backends[0] {
            BackendConfig::Local { tool_filter } => {
                if let ToolFilter::Exclude(excluded_tools) = tool_filter {
                    assert_eq!(excluded_tools.len(), 2);
                    assert!(excluded_tools.contains(&BASH_TOOL_NAME.to_string()));
                    assert!(excluded_tools.contains(&EDIT_TOOL_NAME.to_string()));
                } else {
                    panic!("Expected ToolFilter::Exclude");
                }
            }
            _ => panic!("Expected Local backend config"),
        }
    }

    #[test]
    fn test_session_tool_config_read_only() {
        let config = SessionToolConfig::read_only();
        assert_eq!(config.backends.len(), 0); // Empty backends means use defaults
        assert!(matches!(config.visibility, ToolVisibility::ReadOnly));
        assert!(matches!(
            config.approval_policy,
            ToolApprovalPolicy::AlwaysAsk
        ));
    }

    #[tokio::test]
    async fn test_session_config_build_registry_server_tools() {
        // Test that server tools are properly registered
        let config = SessionConfig {
            workspace: WorkspaceConfig::Local,
            tool_config: SessionToolConfig::default(),
            system_prompt: None,
            metadata: HashMap::new(),
        };

        let registry = config.build_registry().await.unwrap();
        let schemas = registry.get_tool_schemas().await;
        let tool_names: Vec<String> = schemas.iter().map(|s| s.name.clone()).collect();

        // Only server tools should be in the registry
        assert!(tool_names.contains(&"dispatch_agent".to_string()));
        assert!(tool_names.contains(&"web_fetch".to_string()));

        // Verify workspace tools are NOT in the registry (they're handled by Workspace)
        let workspace_tool_names = vec!["bash", "grep", "glob", "ls", "read", "write", "edit"];
        for tool_name in workspace_tool_names {
            assert!(
                !tool_names.contains(&tool_name.to_string()),
                "Workspace tool {} should not be in registry",
                tool_name
            );
        }
    }

    // Test removed: workspace tools are no longer in the registry

    // Test removed: tool visibility filtering for workspace tools happens at the Workspace level

    // Test removed: workspace backend no longer exists in the registry

    #[test]
    fn test_backend_config_variants() {
        // Test Local variant
        let local_config = BackendConfig::Local {
            tool_filter: ToolFilter::Include(vec![
                VIEW_TOOL_NAME.to_string(),
                LS_TOOL_NAME.to_string(),
            ]),
        };

        match local_config {
            BackendConfig::Local { tool_filter } => {
                if let ToolFilter::Include(tools) = tool_filter {
                    assert_eq!(tools.len(), 2);
                } else {
                    panic!("Expected ToolFilter::Include");
                }
            }
            _ => panic!("Expected Local variant"),
        }

        // Test Remote variant
        let remote_config = BackendConfig::Remote {
            name: "test-remote".to_string(),
            endpoint: "http://localhost:8080".to_string(),
            auth: None,
            tool_filter: ToolFilter::All,
        };

        match remote_config {
            BackendConfig::Remote { name, endpoint, .. } => {
                assert_eq!(name, "test-remote");
                assert_eq!(endpoint, "http://localhost:8080");
            }
            _ => panic!("Expected Remote variant"),
        }

        // Test Container variant
        let container_config = BackendConfig::Container {
            image: "ubuntu:latest".to_string(),
            runtime: ContainerRuntime::Docker,
            tool_filter: ToolFilter::All,
        };

        match container_config {
            BackendConfig::Container { image, runtime, .. } => {
                assert_eq!(image, "ubuntu:latest");
                assert!(matches!(runtime, ContainerRuntime::Docker));
            }
            _ => panic!("Expected Container variant"),
        }

        // Test Mcp variant
        let mcp_config = BackendConfig::Mcp {
            server_name: "test-mcp".to_string(),
            transport: "stdio".to_string(),
            command: "python".to_string(),
            args: vec!["-m".to_string(), "test_server".to_string()],
            tool_filter: ToolFilter::All,
        };

        match mcp_config {
            BackendConfig::Mcp {
                server_name,
                transport,
                command,
                args,
                ..
            } => {
                assert_eq!(server_name, "test-mcp");
                assert_eq!(transport, "stdio");
                assert_eq!(command, "python");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected Mcp variant"),
        }
    }
}
