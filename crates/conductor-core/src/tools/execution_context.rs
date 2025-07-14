use crate::config::LlmConfigProvider;
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
    llm_config_provider: Option<LlmConfigProvider>,
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
            llm_config_provider: self.llm_config_provider,
        }
    }
}
