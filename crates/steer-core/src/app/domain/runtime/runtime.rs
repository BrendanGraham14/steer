use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::domain::action::Action;
use crate::app::domain::effect::Effect;
use crate::app::domain::event::SessionEvent;
use crate::app::domain::reduce::reduce;
use crate::app::domain::session::{
    EventStore, EventStoreError, SessionManager, SessionManagerConfig, SessionManagerError,
};
use crate::app::domain::types::{MessageId, NonEmptyString, OpId, RequestId, SessionId};
use crate::config::model::ModelId;
use crate::tools::ToolExecutor;

use super::interpreter::EffectInterpreter;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Session error: {0}")]
    Session(#[from] SessionManagerError),

    #[error("Event store error: {0}")]
    EventStore(#[from] EventStoreError),

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Invalid input: {message}")]
    InvalidInput { message: String },

    #[error("Operation cancelled")]
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub max_active_sessions: usize,
    pub idle_timeout: Duration,
    pub default_model: ModelId,
    pub event_channel_size: usize,
}

impl RuntimeConfig {
    pub fn new(default_model: ModelId) -> Self {
        Self {
            max_active_sessions: 10,
            idle_timeout: Duration::from_secs(300),
            default_model,
            event_channel_size: 256,
        }
    }
}

pub struct AppRuntime {
    session_manager: SessionManager,
    interpreter: EffectInterpreter,
    #[allow(dead_code)]
    config: RuntimeConfig,
    active_operations: HashMap<OpId, CancellationToken>,
    event_tx: mpsc::Sender<(SessionId, SessionEvent)>,
}

impl AppRuntime {
    pub fn new(
        store: Arc<dyn EventStore>,
        api_client: Arc<ApiClient>,
        tool_executor: Arc<ToolExecutor>,
        config: RuntimeConfig,
        event_tx: mpsc::Sender<(SessionId, SessionEvent)>,
    ) -> Result<Self, RuntimeError> {
        let session_config = SessionManagerConfig::new(config.default_model.clone())
            .with_max_active(config.max_active_sessions)
            .with_idle_timeout(config.idle_timeout);

        let session_manager = SessionManager::new(store.clone(), session_config)?;
        let interpreter = EffectInterpreter::new(api_client, tool_executor);

        Ok(Self {
            session_manager,
            interpreter,
            config,
            active_operations: HashMap::new(),
            event_tx,
        })
    }

    pub async fn create_session(&mut self) -> Result<SessionId, RuntimeError> {
        let session_id = SessionId::new();
        self.session_manager.create_session(session_id).await?;
        Ok(session_id)
    }

    pub async fn submit_user_input(
        &mut self,
        session_id: SessionId,
        text: String,
    ) -> Result<OpId, RuntimeError> {
        let text = NonEmptyString::new(text).ok_or_else(|| RuntimeError::InvalidInput {
            message: "Input text cannot be empty".to_string(),
        })?;

        let op_id = OpId::new();
        let message_id = MessageId::new();
        let timestamp = current_timestamp();

        let action = Action::UserInput {
            session_id,
            text,
            op_id,
            message_id,
            timestamp,
        };

        self.dispatch_action(session_id, action).await?;

        let cancel_token = CancellationToken::new();
        self.active_operations.insert(op_id, cancel_token);

        Ok(op_id)
    }

    pub async fn submit_tool_approval(
        &mut self,
        session_id: SessionId,
        request_id: RequestId,
        approved: bool,
        remember_tool: Option<String>,
        remember_pattern: Option<String>,
    ) -> Result<(), RuntimeError> {
        use crate::app::domain::action::{ApprovalDecision, ApprovalMemory};

        let decision = if approved {
            ApprovalDecision::Approved
        } else {
            ApprovalDecision::Denied
        };

        let remember = if let Some(tool) = remember_tool {
            Some(ApprovalMemory::Tool(tool))
        } else if let Some(pattern) = remember_pattern {
            Some(ApprovalMemory::BashPattern(pattern))
        } else {
            None
        };

        let action = Action::ToolApprovalDecided {
            session_id,
            request_id,
            decision,
            remember,
        };

        self.dispatch_action(session_id, action).await
    }

    pub async fn cancel_operation(
        &mut self,
        session_id: SessionId,
        op_id: Option<OpId>,
    ) -> Result<(), RuntimeError> {
        if let Some(target_op) = op_id {
            if let Some(token) = self.active_operations.get(&target_op) {
                token.cancel();
            }
        } else {
            for token in self.active_operations.values() {
                token.cancel();
            }
        }

        let action = Action::Cancel { session_id, op_id };
        self.dispatch_action(session_id, action).await
    }

    pub fn evict_idle_sessions(&mut self) -> usize {
        self.session_manager.evict_idle()
    }

    pub fn active_session_count(&self) -> usize {
        self.session_manager.active_count()
    }

    async fn dispatch_action(
        &mut self,
        session_id: SessionId,
        action: Action,
    ) -> Result<(), RuntimeError> {
        let mut pending_actions = vec![action];

        while let Some(current_action) = pending_actions.pop() {
            let session = self.session_manager.get_session(session_id).await?;
            let effects = reduce(&mut session.state, current_action);

            for effect in effects {
                let new_actions = self.interpret_effect(session_id, effect).await?;
                pending_actions.extend(new_actions);
            }
        }

        Ok(())
    }

    async fn interpret_effect(
        &mut self,
        session_id: SessionId,
        effect: Effect,
    ) -> Result<Vec<Action>, RuntimeError> {
        match effect {
            Effect::EmitEvent { event, .. } => {
                self.session_manager.persist_event(session_id, &event).await?;

                self.event_tx
                    .send((session_id, event))
                    .await
                    .map_err(|_| RuntimeError::ChannelClosed)?;

                Ok(vec![])
            }

            Effect::CallModel {
                op_id,
                model,
                messages,
                system_prompt,
                tools,
                ..
            } => {
                let cancel_token = self
                    .active_operations
                    .get(&op_id)
                    .cloned()
                    .unwrap_or_else(CancellationToken::new);

                let result = self
                    .interpreter
                    .call_model(model, messages, system_prompt, tools, cancel_token)
                    .await;

                let action = match result {
                    Ok(content) => {
                        let message_id = MessageId::new();
                        let timestamp = current_timestamp();

                        Action::ModelResponseComplete {
                            session_id,
                            op_id,
                            message_id,
                            content,
                            timestamp,
                        }
                    }
                    Err(e) => Action::ModelResponseError {
                        session_id,
                        op_id,
                        error: e.to_string(),
                    },
                };

                Ok(vec![action])
            }

            Effect::ExecuteTool {
                op_id, tool_call, ..
            } => {
                let cancel_token = self
                    .active_operations
                    .get(&op_id)
                    .cloned()
                    .unwrap_or_else(CancellationToken::new);

                let tool_call_id = crate::app::domain::types::ToolCallId::from_string(&tool_call.id);

                let start_action = Action::ToolExecutionStarted {
                    session_id,
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    tool_parameters,
                };

                let result = self
                    .interpreter
                    .execute_tool(tool_call, cancel_token)
                    .await;

                let result_action = Action::ToolResult {
                    session_id,
                    tool_call_id,
                    tool_name,
                    result,
                };

                Ok(vec![start_action, result_action])
            }

            Effect::RequestUserApproval {
                request_id,
                tool_call,
                ..
            } => {
                let event = SessionEvent::ApprovalRequested {
                    request_id,
                    tool_call,
                };
                self.event_tx
                    .send((session_id, event))
                    .await
                    .map_err(|_| RuntimeError::ChannelClosed)?;

                Ok(vec![])
            }

            Effect::CancelOperation { op_id, .. } => {
                if let Some(token) = self.active_operations.remove(&op_id) {
                    token.cancel();
                }
                Ok(vec![])
            }

            Effect::ListWorkspaceFiles { .. } => Ok(vec![]),

            Effect::ConnectMcpServer { .. } | Effect::DisconnectMcpServer { .. } => Ok(vec![]),
        }
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::domain::session::InMemoryEventStore;
    use crate::config::model::builtin;

    fn test_config() -> RuntimeConfig {
        RuntimeConfig::new(builtin::claude_sonnet_4_20250514())
    }

    #[test]
    fn test_runtime_config_defaults() {
        let config = test_config();
        assert_eq!(config.max_active_sessions, 10);
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
        assert_eq!(config.event_channel_size, 256);
    }

    #[test]
    fn test_session_manager_config_from_runtime_config() {
        let config = test_config();
        let session_config = SessionManagerConfig::new(config.default_model.clone())
            .with_max_active(config.max_active_sessions)
            .with_idle_timeout(config.idle_timeout);

        assert_eq!(session_config.max_active_sessions, 10);
    }
}
