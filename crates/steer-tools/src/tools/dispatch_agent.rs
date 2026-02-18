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
    /// Run the sub-agent in the caller's current workspace.
    Current,
    /// Create a fresh workspace (jj workspace or git worktree) and run there.
    /// The resulting path may differ from the caller's current directory.
    New { name: String },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(tag = "session", rename_all = "snake_case")]
pub enum DispatchAgentTarget {
    /// Start a new child session.
    New {
        workspace: WorkspaceTarget,
        #[serde(default)]
        agent: Option<String>,
    },
    /// Continue an existing child session by id.
    Resume { session_id: String },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct DispatchAgentParams {
    /// Instructions for the sub-agent.
    /// Include relevant context you already gathered (paths, findings,
    /// constraints, and acceptance criteria) so the sub-agent does not need to
    /// re-gather it.
    /// Do not prepend synthetic path headers like `Repo: ...` or `CWD: ...`.
    /// The sub-agent receives its working-directory context automatically.
    pub prompt: String,
    /// Session/workspace target for the sub-agent call.
    pub target: DispatchAgentTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
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
