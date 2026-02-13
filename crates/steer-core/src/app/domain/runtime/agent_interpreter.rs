use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::conversation::Message;
use crate::app::domain::action::ApprovalDecision;
use crate::app::domain::event::{CancellationInfo, OperationKind, SessionEvent};
use crate::app::domain::session::EventStore;
use crate::app::domain::types::{MessageId, OpId, RequestId, SessionId, ToolCallId};
use crate::config::model::builtin::default_model;
use crate::session::state::{
    SessionConfig, SessionPolicyOverrides, SessionToolConfig, ToolApprovalPolicyOverrides,
    ToolVisibility, WorkspaceConfig,
};
use crate::tools::{SessionMcpBackends, ToolExecutor};

use super::interpreter::EffectInterpreter;
use super::stepper::{AgentConfig, AgentInput, AgentOutput, AgentState, AgentStepper};

#[derive(Clone, Default)]
pub struct AgentInterpreterConfig {
    pub auto_approve_tools: bool,
    pub parent_session_id: Option<SessionId>,
    pub session_config: Option<SessionConfig>,
    pub session_backends: Option<Arc<SessionMcpBackends>>,
}

impl std::fmt::Debug for AgentInterpreterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentInterpreterConfig")
            .field("auto_approve_tools", &self.auto_approve_tools)
            .field("parent_session_id", &self.parent_session_id)
            .field("session_config", &self.session_config)
            .field("session_backends", &self.session_backends.is_some())
            .finish()
    }
}

impl AgentInterpreterConfig {
    pub fn for_sub_agent(parent_session_id: SessionId) -> Self {
        Self {
            auto_approve_tools: true,
            parent_session_id: Some(parent_session_id),
            session_config: None,
            session_backends: None,
        }
    }
}

pub struct AgentInterpreter {
    session_id: SessionId,
    op_id: OpId,
    config: AgentInterpreterConfig,
    event_store: Arc<dyn EventStore>,
    effect_interpreter: EffectInterpreter,
}

impl AgentInterpreter {
    pub async fn new(
        event_store: Arc<dyn EventStore>,
        api_client: Arc<ApiClient>,
        tool_executor: Arc<ToolExecutor>,
        config: AgentInterpreterConfig,
    ) -> Result<Self, AgentInterpreterError> {
        let session_id = SessionId::new();
        let op_id = OpId::new();

        event_store
            .create_session(session_id)
            .await
            .map_err(|e| AgentInterpreterError::EventStore(e.to_string()))?;

        let mut session_config = config
            .session_config
            .clone()
            .unwrap_or_else(|| default_session_config(default_model()));
        if session_config.parent_session_id.is_none() {
            session_config.parent_session_id = config.parent_session_id;
        }

        let session_created_event = SessionEvent::SessionCreated {
            config: Box::new(session_config),
            metadata: HashMap::new(),
            parent_session_id: config.parent_session_id,
        };
        event_store
            .append(session_id, &session_created_event)
            .await
            .map_err(|e| AgentInterpreterError::EventStore(e.to_string()))?;

        let mut effect_interpreter =
            EffectInterpreter::new(api_client, tool_executor).with_session(session_id);
        if let Some(backends) = config.session_backends.clone() {
            effect_interpreter = effect_interpreter.with_session_backends(backends);
        }

        Ok(Self {
            session_id,
            op_id,
            config,
            event_store,
            effect_interpreter,
        })
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub async fn run(
        &self,
        agent_config: AgentConfig,
        initial_messages: Vec<Message>,
        message_tx: Option<mpsc::Sender<Message>>,
        cancel_token: CancellationToken,
    ) -> Result<Message, AgentInterpreterError> {
        self.emit_event(SessionEvent::OperationStarted {
            op_id: self.op_id,
            kind: OperationKind::AgentLoop,
        })
        .await?;

        let stepper = AgentStepper::new(agent_config.clone());
        let mut state = AgentStepper::initial_state(initial_messages.clone());

        let initial_outputs = vec![AgentOutput::CallModel {
            model: agent_config.model.clone(),
            messages: initial_messages,
            system_context: Box::new(agent_config.system_context.clone()),
            tools: agent_config.tools.clone(),
        }];

        let mut pending_outputs: VecDeque<AgentOutput> = VecDeque::from(initial_outputs);

        loop {
            if cancel_token.is_cancelled()
                && !matches!(state, AgentState::Cancelled)
                && !stepper.is_terminal(&state)
            {
                let (new_state, outputs) = stepper.step(state, AgentInput::Cancel);
                state = new_state;
                pending_outputs = VecDeque::from(outputs);
            }

            let output = if let Some(o) = pending_outputs.pop_front() {
                o
            } else {
                if stepper.is_terminal(&state) {
                    match state {
                        AgentState::Complete { final_message } => {
                            self.emit_event(SessionEvent::OperationCompleted { op_id: self.op_id })
                                .await?;
                            return Ok(final_message);
                        }
                        AgentState::Failed { error } => {
                            self.emit_event(SessionEvent::Error {
                                message: error.clone(),
                            })
                            .await?;
                            self.emit_event(SessionEvent::OperationCompleted { op_id: self.op_id })
                                .await?;
                            return Err(AgentInterpreterError::Agent(error));
                        }
                        AgentState::Cancelled => {
                            self.emit_event(SessionEvent::OperationCancelled {
                                op_id: self.op_id,
                                info: CancellationInfo {
                                    pending_tool_calls: 0,
                                    popped_queued_item: None,
                                },
                            })
                            .await?;
                            return Err(AgentInterpreterError::Cancelled);
                        }
                        _ => unreachable!(),
                    }
                }
                return Err(AgentInterpreterError::Agent(
                    "Stepper stuck with no outputs".to_string(),
                ));
            };

            match output {
                AgentOutput::CallModel {
                    model,
                    messages,
                    system_context,
                    tools,
                } => {
                    let result = self
                        .effect_interpreter
                        .call_model(
                            model.clone(),
                            messages,
                            *system_context,
                            tools,
                            cancel_token.clone(),
                        )
                        .await;

                    let message_id = MessageId::new();
                    let timestamp = current_timestamp();

                    let input = match result {
                        Ok(response) => {
                            let tool_calls: Vec<_> = response
                                .content
                                .iter()
                                .filter_map(|c| {
                                    if let crate::app::conversation::AssistantContent::ToolCall {
                                        tool_call,
                                        ..
                                    } = c
                                    {
                                        Some(tool_call.clone())
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            AgentInput::ModelResponse {
                                content: response.content,
                                tool_calls,
                                message_id,
                                timestamp,
                            }
                        }
                        Err(error) => AgentInput::ModelError { error },
                    };

                    let (new_state, outputs) = stepper.step(state, input);
                    state = new_state;
                    pending_outputs.extend(outputs);
                }

                AgentOutput::RequestApproval { tool_call } => {
                    let tool_call_id = ToolCallId::from_string(&tool_call.id);
                    let request_id = RequestId::new();

                    self.emit_event(SessionEvent::ApprovalRequested {
                        request_id,
                        tool_call: tool_call.clone(),
                    })
                    .await?;

                    if !self.config.auto_approve_tools {
                        return Err(AgentInterpreterError::Agent(
                            "Interactive tool approval not supported in AgentInterpreter".into(),
                        ));
                    }

                    self.emit_event(SessionEvent::ApprovalDecided {
                        request_id,
                        decision: ApprovalDecision::Approved,
                        remember: None,
                    })
                    .await?;

                    let input = AgentInput::ToolApproved { tool_call_id };

                    let (new_state, outputs) = stepper.step(state, input);
                    state = new_state;
                    pending_outputs.extend(outputs);
                }

                AgentOutput::ExecuteTool { tool_call } => {
                    let tool_call_id = ToolCallId::from_string(&tool_call.id);

                    self.emit_event(SessionEvent::ToolCallStarted {
                        id: tool_call_id.clone(),
                        name: tool_call.name.clone(),
                        parameters: tool_call.parameters.clone(),
                        model: agent_config.model.clone(),
                    })
                    .await?;

                    let result = self
                        .effect_interpreter
                        .execute_tool(tool_call.clone(), cancel_token.clone())
                        .await;

                    let message_id = MessageId::new();
                    let timestamp = current_timestamp();

                    let input = match result {
                        Ok(tool_result) => {
                            self.emit_event(SessionEvent::ToolCallCompleted {
                                id: tool_call_id.clone(),
                                name: tool_call.name.clone(),
                                result: tool_result.clone(),
                                model: agent_config.model.clone(),
                            })
                            .await?;

                            AgentInput::ToolCompleted {
                                tool_call_id,
                                result: tool_result,
                                message_id,
                                timestamp,
                            }
                        }
                        Err(error) => {
                            self.emit_event(SessionEvent::ToolCallFailed {
                                id: tool_call_id.clone(),
                                name: tool_call.name.clone(),
                                error: error.to_string(),
                                model: agent_config.model.clone(),
                            })
                            .await?;

                            AgentInput::ToolFailed {
                                tool_call_id,
                                error,
                                message_id,
                                timestamp,
                            }
                        }
                    };

                    let (new_state, outputs) = stepper.step(state, input);
                    state = new_state;
                    pending_outputs.extend(outputs);
                }

                AgentOutput::EmitMessage { message } => {
                    self.emit_event(SessionEvent::AssistantMessageAdded {
                        message: message.clone(),
                        model: agent_config.model.clone(),
                    })
                    .await?;

                    if let Some(ref tx) = message_tx {
                        let _ = tx.send(message).await;
                    }
                }

                AgentOutput::Done { final_message } => {
                    self.emit_event(SessionEvent::OperationCompleted { op_id: self.op_id })
                        .await?;
                    return Ok(final_message);
                }

                AgentOutput::Error { error } => {
                    self.emit_event(SessionEvent::Error {
                        message: error.clone(),
                    })
                    .await?;
                    self.emit_event(SessionEvent::OperationCompleted { op_id: self.op_id })
                        .await?;
                    return Err(AgentInterpreterError::Agent(error));
                }

                AgentOutput::Cancelled => {
                    self.emit_event(SessionEvent::OperationCancelled {
                        op_id: self.op_id,
                        info: CancellationInfo {
                            pending_tool_calls: 0,
                            popped_queued_item: None,
                        },
                    })
                    .await?;
                    return Err(AgentInterpreterError::Cancelled);
                }
            }
        }
    }

    async fn emit_event(&self, event: SessionEvent) -> Result<(), AgentInterpreterError> {
        self.event_store
            .append(self.session_id, &event)
            .await
            .map_err(|e| AgentInterpreterError::EventStore(e.to_string()))?;
        Ok(())
    }
}

fn default_session_config(default_model: crate::config::model::ModelId) -> SessionConfig {
    SessionConfig {
        workspace: WorkspaceConfig::Local {
            path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        },
        workspace_ref: None,
        workspace_id: None,
        repo_ref: None,
        parent_session_id: None,
        workspace_name: None,
        tool_config: SessionToolConfig {
            backends: Vec::new(),
            visibility: ToolVisibility::All,
            approval_policy: crate::session::state::ToolApprovalPolicy::default(),
            metadata: HashMap::new(),
        },
        system_prompt: None,
        primary_agent_id: None,
        policy_overrides: SessionPolicyOverrides {
            default_model: None,
            tool_visibility: Some(ToolVisibility::ReadOnly),
            approval_policy: ToolApprovalPolicyOverrides::empty(),
        },
        metadata: HashMap::new(),
        default_model,
        auto_compaction: crate::session::state::AutoCompactionConfig::default(),
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Error)]
pub enum AgentInterpreterError {
    #[error("API error: {0}")]
    Api(String),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("Event store error: {0}")]
    EventStore(String),

    #[error("Cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::ApiError;
    use crate::api::provider::{CompletionResponse, Provider};
    use crate::app::SystemContext;
    use crate::app::conversation::AssistantContent;
    use crate::app::domain::session::event_store::InMemoryEventStore;
    use crate::app::validation::ValidatorRegistry;
    use crate::auth::ProviderRegistry;
    use crate::config::model::ModelId;
    use crate::config::provider::ProviderId;
    use crate::model_registry::ModelRegistry;
    use crate::tools::BackendRegistry;
    use async_trait::async_trait;
    use steer_tools::ToolSchema;

    #[derive(Clone)]
    struct StubProvider {
        cancel_on_complete: bool,
    }

    #[async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &'static str {
            "stub"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<crate::config::model::ModelParameters>,
            token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            if self.cancel_on_complete {
                token.cancel();
            }

            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: "ok".to_string(),
                }],
                usage: None,
            })
        }
    }

    async fn create_test_deps() -> (Arc<dyn EventStore>, Arc<ApiClient>, Arc<ToolExecutor>) {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).expect("model registry"));
        let provider_registry = Arc::new(ProviderRegistry::load(&[]).expect("provider registry"));
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider().unwrap(),
            provider_registry,
            model_registry,
        ));

        let tool_executor = Arc::new(ToolExecutor::with_components(
            Arc::new(BackendRegistry::new()),
            Arc::new(ValidatorRegistry::new()),
        ));

        (event_store, api_client, tool_executor)
    }

    #[tokio::test]
    async fn test_cancel_after_completion_does_not_override_outputs() {
        let (event_store, api_client, tool_executor) = create_test_deps().await;
        let provider_id = ProviderId("stub".to_string());
        let model_id = ModelId::new(provider_id.clone(), "stub-model");
        api_client.insert_test_provider(
            provider_id,
            Arc::new(StubProvider {
                cancel_on_complete: true,
            }),
        );

        let interpreter = AgentInterpreter::new(
            event_store.clone(),
            api_client,
            tool_executor,
            AgentInterpreterConfig::default(),
        )
        .await
        .expect("interpreter");

        let cancel_token = CancellationToken::new();
        let result = interpreter
            .run(
                AgentConfig {
                    model: model_id,
                    system_context: None,
                    tools: vec![],
                },
                vec![],
                None,
                cancel_token.clone(),
            )
            .await;

        assert!(result.is_ok(), "expected run to complete, got {result:?}");
        assert!(cancel_token.is_cancelled(), "cancel token should be set");

        let events = event_store
            .load_events(interpreter.session_id())
            .await
            .expect("load events");

        assert!(
            events
                .iter()
                .any(|(_, event)| matches!(event, SessionEvent::AssistantMessageAdded { .. })),
            "assistant message should be emitted"
        );
        assert!(
            events
                .iter()
                .any(|(_, event)| matches!(event, SessionEvent::OperationCompleted { .. })),
            "operation should complete"
        );
        assert!(
            !events
                .iter()
                .any(|(_, event)| matches!(event, SessionEvent::OperationCancelled { .. })),
            "operation should not be cancelled"
        );
    }
}
