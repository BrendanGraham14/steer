use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Core execution context passed to all tool executions
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub session_id: String,
    pub operation_id: String,
    pub tool_call_id: String,
    pub cancellation_token: CancellationToken,
    pub timeout: Duration,
    pub trace_context: TraceContext,
    pub environment: ExecutionEnvironment,
}

/// Defines the execution environment for tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionEnvironment {
    /// Execute tools locally in the current process
    Local,

    /// Execute tools on a remote machine via an agent
    Remote {
        agent_address: String,
        auth_method: AuthMethod,
        working_directory: Option<String>,
    },
}

/// Authentication methods for remote execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    /// No authentication
    None,
}

/// Volume mount for container execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMount {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

/// Observability context for distributed tracing
#[derive(Debug, Clone)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub baggage: HashMap<String, String>,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            session_id: "default".to_string(),
            operation_id: "default".to_string(),
            tool_call_id: "default".to_string(),
            cancellation_token: CancellationToken::new(),
            timeout: Duration::from_secs(300), // 5 minutes default
            trace_context: TraceContext::default(),
            environment: ExecutionEnvironment::Local,
        }
    }
}

impl Default for TraceContext {
    fn default() -> Self {
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            span_id: uuid::Uuid::new_v4().to_string(),
            parent_span_id: None,
            baggage: HashMap::new(),
        }
    }
}

impl ExecutionContext {
    pub fn new(
        session_id: String,
        operation_id: String,
        tool_call_id: String,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            session_id,
            operation_id,
            tool_call_id,
            cancellation_token,
            timeout: Duration::from_secs(300),
            trace_context: TraceContext::default(),
            environment: ExecutionEnvironment::Local,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_environment(mut self, environment: ExecutionEnvironment) -> Self {
        self.environment = environment;
        self
    }

    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = trace_context;
        self
    }
}

impl TraceContext {
    pub fn new(trace_id: String, span_id: String) -> Self {
        Self {
            trace_id,
            span_id,
            parent_span_id: None,
            baggage: HashMap::new(),
        }
    }

    pub fn with_parent(mut self, parent_span_id: String) -> Self {
        self.parent_span_id = Some(parent_span_id);
        self
    }

    pub fn with_baggage(mut self, key: String, value: String) -> Self {
        self.baggage.insert(key, value);
        self
    }
}

impl Default for ExecutionEnvironment {
    fn default() -> Self {
        Self::Local
    }
}

impl Default for AuthMethod {
    fn default() -> Self {
        Self::None
    }
}
