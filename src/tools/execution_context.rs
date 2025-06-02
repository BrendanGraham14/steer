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
