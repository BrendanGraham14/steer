use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::conversation::Message;
use crate::app::domain::action::{ApprovalDecision, ApprovalMemory};
use crate::app::domain::event::{CancellationInfo, OperationKind, SessionEvent};
use crate::app::domain::session::EventStore;
use crate::app::domain::types::{MessageId, OpId, RequestId, SessionId, ToolCallId};
use crate::session::state::SessionConfig;
use crate::tools::ToolExecutor;

use super::interpreter::EffectInterpreter;
use super::stepper::{AgentConfig, AgentInput, AgentOutput, AgentState, AgentStepper};

#[derive(Debug, Clone)]
pub struct AgentInterpreterConfig {
    pub auto_approve_tools: bool,
    pub parent_session_id: Option<SessionId>,
}

impl Default for AgentInterpreterConfig {
    fn default() -> Self {
        Self {
            auto_approve_tools: false,
            parent_session_id: None,
        }
    }
}

impl AgentInterpreterConfig {
    pub fn for_sub_agent(parent_session_id: SessionId) -> Self {
        Self {
            auto_approve_tools: true,
            parent_session_id: Some(parent_session_id),
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

        let session_created_event = SessionEvent::SessionCreated {
            config: SessionConfig::read_only(),
            metadata: HashMap::new(),
            parent_session_id: config.parent_session_id,
        };
        event_store
            .append(session_id, &session_created_event)
            .await
            .map_err(|e| AgentInterpreterError::EventStore(e.to_string()))?;

        let effect_interpreter =
            EffectInterpreter::new(api_client, tool_executor).with_session(session_id);

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
            system_prompt: agent_config.system_prompt.clone(),
            tools: agent_config.tools.clone(),
        }];

        let mut pending_outputs = initial_outputs;

        loop {
            if cancel_token.is_cancelled() {
                self.emit_event(SessionEvent::OperationCancelled {
                    op_id: self.op_id,
                    info: CancellationInfo {
                        pending_tool_calls: 0,
                    },
                })
                .await?;
                return Err(AgentInterpreterError::Cancelled);
            }

            let output = match pending_outputs.pop() {
                Some(o) => o,
                None => {
                    if stepper.is_terminal(&state) {
                        match state {
                            AgentState::Complete { final_message } => {
                                self.emit_event(SessionEvent::OperationCompleted {
                                    op_id: self.op_id,
                                })
                                .await?;
                                return Ok(final_message);
                            }
                            AgentState::Failed { error } => {
                                self.emit_event(SessionEvent::Error {
                                    message: error.clone(),
                                })
                                .await?;
                                self.emit_event(SessionEvent::OperationCompleted {
                                    op_id: self.op_id,
                                })
                                .await?;
                                return Err(AgentInterpreterError::Agent(error));
                            }
                            AgentState::Cancelled => {
                                self.emit_event(SessionEvent::OperationCancelled {
                                    op_id: self.op_id,
                                    info: CancellationInfo {
                                        pending_tool_calls: 0,
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
                }
            };

            match output {
                AgentOutput::CallModel {
                    model,
                    messages,
                    system_prompt,
                    tools,
                } => {
                    let result = self
                        .effect_interpreter
                        .call_model(
                            model.clone(),
                            messages,
                            system_prompt,
                            tools,
                            cancel_token.clone(),
                        )
                        .await;

                    let message_id = MessageId::new();
                    let timestamp = current_timestamp();

                    let input = match result {
                        Ok(content) => {
                            let tool_calls: Vec<_> = content
                                .iter()
                                .filter_map(|c| {
                                    if let crate::app::conversation::AssistantContent::ToolCall {
                                        tool_call,
                                    } = c
                                    {
                                        Some(tool_call.clone())
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            AgentInput::ModelResponse {
                                content,
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

                    let decision = ApprovalDecision::Approved;
                    let remember: Option<ApprovalMemory> = None;

                    self.emit_event(SessionEvent::ApprovalDecided {
                        request_id,
                        decision,
                        remember,
                    })
                    .await?;

                    let input = if decision == ApprovalDecision::Approved {
                        AgentInput::ToolApproved { tool_call_id }
                    } else {
                        AgentInput::ToolDenied { tool_call_id }
                    };

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
                    self.emit_event(SessionEvent::MessageAdded {
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
