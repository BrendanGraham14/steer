use crate::auth::ProviderRegistry;
use crate::catalog::CatalogConfig;
use crate::config::LlmConfigProvider;
use crate::config::model::ModelId;
use crate::error::Result;
use crate::model_registry::ModelRegistry;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use steer_tools::ToolResult;

pub mod adapters;
mod agent_executor;
pub mod cancellation;
pub mod command;
pub mod context;
pub mod conversation;
pub mod domain;
pub mod io;

pub mod validation;

pub use cancellation::CancellationInfo;
pub use command::AppCommand;
pub use context::OpContext;
pub use conversation::{Conversation, Message, MessageData};
pub use steer_workspace::EnvironmentInfo;

pub use agent_executor::{
    AgentEvent, AgentExecutor, AgentExecutorError, AgentExecutorRunRequest, ApprovalDecision,
};

#[derive(Debug, Clone)]
pub enum AppEvent {
    MessageAdded {
        message: Message,
        model: ModelId,
    },
    MessageUpdated {
        id: String,
        content: String,
    },
    MessagePart {
        id: String,
        delta: String,
    },

    ToolCallStarted {
        name: String,
        id: String,
        parameters: serde_json::Value,
        model: ModelId,
    },
    ToolCallCompleted {
        name: String,
        result: ToolResult,
        id: String,
        model: ModelId,
    },
    ToolCallFailed {
        name: String,
        error: String,
        id: String,
        model: ModelId,
    },

    ProcessingStarted,
    ProcessingCompleted,

    CommandResponse {
        command: conversation::AppCommandType,
        response: conversation::CommandResponse,
        id: String,
    },

    RequestToolApproval {
        name: String,
        parameters: serde_json::Value,
        id: String,
    },
    OperationCancelled {
        op_id: Option<uuid::Uuid>,
        info: CancellationInfo,
    },

    ModelChanged {
        model: ModelId,
    },
    Error {
        message: String,
    },
    WorkspaceChanged,
    WorkspaceFiles {
        files: Vec<String>,
    },
    Started {
        id: uuid::Uuid,
        op: Operation,
    },
    Finished {
        id: uuid::Uuid,
        outcome: OperationOutcome,
    },
    ActiveMessageIdChanged {
        message_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Operation {
    Bash { cmd: String },
    Compact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationOutcome {
    Bash {
        elapsed: Duration,
        result: std::result::Result<(), BashError>,
    },
    Compact {
        elapsed: Duration,
        result: std::result::Result<(), CompactError>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashError {
    pub exit_code: i32,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactError {
    pub message: String,
}

#[derive(Clone)]
pub struct AppConfig {
    pub llm_config_provider: LlmConfigProvider,
    pub model_registry: Arc<ModelRegistry>,
    pub provider_registry: Arc<ProviderRegistry>,
}

impl AppConfig {
    pub fn from_auth_storage(auth_storage: Arc<dyn crate::auth::AuthStorage>) -> Result<Self> {
        Self::from_auth_storage_with_catalog(auth_storage, CatalogConfig::default())
    }

    pub fn from_auth_storage_with_catalog(
        auth_storage: Arc<dyn crate::auth::AuthStorage>,
        catalog_config: CatalogConfig,
    ) -> Result<Self> {
        let llm_config_provider = LlmConfigProvider::new(auth_storage);
        let model_registry = Arc::new(ModelRegistry::load(&catalog_config.catalog_paths)?);
        let provider_registry = Arc::new(ProviderRegistry::load(&catalog_config.catalog_paths)?);

        Ok(Self {
            llm_config_provider,
            model_registry,
            provider_registry,
        })
    }

    #[cfg(not(test))]
    pub fn new() -> Result<Self> {
        let auth_storage = Arc::new(crate::auth::DefaultAuthStorage::new()?);
        Self::from_auth_storage(auth_storage)
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        let storage = Arc::new(crate::test_utils::InMemoryAuthStorage::new());
        Self::from_auth_storage(storage).expect("Failed to create test AppConfig")
    }
}
