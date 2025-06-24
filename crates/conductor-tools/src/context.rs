use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

/// Execution context passed to tools during execution
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Unique identifier for this tool call
    pub tool_call_id: String,

    /// Cancellation token for early termination
    pub cancellation_token: CancellationToken,

    /// Current working directory
    pub working_directory: std::path::PathBuf,

    /// Environment variables available to the tool
    pub environment: HashMap<String, String>,

    /// Execution metadata
    pub metadata: HashMap<String, String>,
}

impl ExecutionContext {
    pub fn new(tool_call_id: String) -> Self {
        Self {
            tool_call_id,
            cancellation_token: CancellationToken::new(),
            working_directory: std::env::current_dir().unwrap_or_else(|_| "/".into()),
            environment: std::env::vars().collect(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_cancellation_token(mut self, token: CancellationToken) -> Self {
        self.cancellation_token = token;
        self
    }

    pub fn with_working_directory(mut self, dir: std::path::PathBuf) -> Self {
        self.working_directory = dir;
        self
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }
}
