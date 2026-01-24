use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::{ToolExecutionError, WorkspaceOpError};
use crate::result::AgentResult;

pub const DISPATCH_AGENT_TOOL_NAME: &str = "dispatch_agent";

pub struct DispatchAgentToolSpec;

impl ToolSpec for DispatchAgentToolSpec {
    type Params = DispatchAgentParams;
    type Result = AgentResult;
    type Error = DispatchAgentError;

    const NAME: &'static str = DISPATCH_AGENT_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Dispatch Agent";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::DispatchAgent(error)
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(tag = "location", rename_all = "snake_case")]
pub enum WorkspaceTarget {
    Current,
    New { name: String },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(tag = "session", rename_all = "snake_case")]
pub enum DispatchAgentTarget {
    New {
        workspace: WorkspaceTarget,
        #[serde(default)]
        agent: Option<String>,
    },
    Resume {
        session_id: String,
    },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct DispatchAgentParams {
    pub prompt: String,
    pub target: DispatchAgentTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum DispatchAgentError {
    #[error("{0}")]
    Workspace(WorkspaceOpError),

    #[error("workspace unavailable: {message}")]
    WorkspaceUnavailable { message: String },

    #[error("sub-agent failed: {message}")]
    SpawnFailed { message: String },

    #[error("failed to load session {session_id}: {message}")]
    SessionLoadFailed { session_id: String, message: String },

    #[error("session {session_id} is missing a SessionCreated event")]
    MissingSessionCreatedEvent { session_id: String },

    #[error("session {session_id} is not a child of current session {parent_session_id}")]
    InvalidParentSession {
        session_id: String,
        parent_session_id: String,
    },

    #[error("failed to open workspace for session {session_id}: {message}")]
    WorkspaceOpenFailed { session_id: String, message: String },
}
