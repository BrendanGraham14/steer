use crate::config::LlmConfigProvider;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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
    pub llm_config_provider: Option<LlmConfigProvider>,
}

/// Builder for ExecutionContext
#[derive(Debug)]
pub struct ExecutionContextBuilder {
    session_id: String,
    operation_id: String,
    tool_call_id: String,
    cancellation_token: CancellationToken,
    timeout: Duration,
    environment: ExecutionEnvironment,
    llm_config_provider: Option<LlmConfigProvider>,
}

/// Defines the execution environment for tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionEnvironment {
    /// Execute tools locally in the current process
    Local { working_directory: PathBuf },

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
    /// Create a new builder for ExecutionContext
    pub fn builder(
        session_id: String,
        operation_id: String,
        tool_call_id: String,
        cancellation_token: CancellationToken,
    ) -> ExecutionContextBuilder {
        ExecutionContextBuilder {
            session_id,
            operation_id,
            tool_call_id,
            cancellation_token,
            timeout: Duration::from_secs(300),
            environment: ExecutionEnvironment::default(),
            llm_config_provider: None,
        }
    }

    /// Legacy constructor - prefer using builder() instead
    pub fn new(
        session_id: String,
        operation_id: String,
        tool_call_id: String,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self::builder(session_id, operation_id, tool_call_id, cancellation_token).build()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_environment(mut self, environment: ExecutionEnvironment) -> Self {
        self.environment = environment;
        self
    }

    pub fn with_llm_config_provider(mut self, provider: LlmConfigProvider) -> Self {
        self.llm_config_provider = Some(provider);
        self
    }
}

impl ExecutionContextBuilder {
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn environment(mut self, environment: ExecutionEnvironment) -> Self {
        self.environment = environment;
        self
    }

    pub fn llm_config_provider(mut self, provider: LlmConfigProvider) -> Self {
        self.llm_config_provider = Some(provider);
        self
    }

    pub fn build(self) -> ExecutionContext {
        ExecutionContext {
            session_id: self.session_id,
            operation_id: self.operation_id,
            tool_call_id: self.tool_call_id,
            cancellation_token: self.cancellation_token,
            timeout: self.timeout,
            environment: self.environment,
            llm_config_provider: self.llm_config_provider,
        }
    }
}

impl Default for ExecutionEnvironment {
    fn default() -> Self {
        Self::Local {
            working_directory: crate::utils::default_working_directory(),
        }
    }
}

impl Default for AuthMethod {
    fn default() -> Self {
        Self::None
    }
}
