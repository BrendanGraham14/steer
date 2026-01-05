use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::conversation::Message;
use crate::app::domain::types::{MessageId, ToolCallId};
use crate::tools::ToolExecutor;

use super::stepper::{AgentConfig, AgentInput, AgentOutput, AgentState, AgentStepper};

pub struct AgentInterpreter {
    api_client: Arc<ApiClient>,
    tool_executor: Arc<ToolExecutor>,
}

impl AgentInterpreter {
    pub fn new(api_client: Arc<ApiClient>, tool_executor: Arc<ToolExecutor>) -> Self {
        Self {
            api_client,
            tool_executor,
        }
    }

    pub async fn run(
        &self,
        config: AgentConfig,
        initial_messages: Vec<Message>,
        message_tx: mpsc::Sender<Message>,
        cancel_token: CancellationToken,
    ) -> Result<Message, AgentInterpreterError> {
        let stepper = AgentStepper::new(config.clone());
        let mut state = AgentStepper::initial_state(initial_messages.clone());

        let initial_outputs = vec![AgentOutput::CallModel {
            model: config.model.clone(),
            messages: initial_messages,
            system_prompt: config.system_prompt.clone(),
            tools: config.tools.clone(),
        }];

        let mut pending_outputs = initial_outputs;

        loop {
            if cancel_token.is_cancelled() {
                return Err(AgentInterpreterError::Cancelled);
            }

            // Process one output at a time to avoid borrow conflicts
            let output = match pending_outputs.pop() {
                Some(o) => o,
                None => {
                    if stepper.is_terminal(&state) {
                        match state {
                            AgentState::Complete { final_message } => return Ok(final_message),
                            AgentState::Failed { error } => {
                                return Err(AgentInterpreterError::Agent(error));
                            }
                            AgentState::Cancelled => return Err(AgentInterpreterError::Cancelled),
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
                    let tools_option = if tools.is_empty() { None } else { Some(tools) };

                    let response = self
                        .api_client
                        .complete_with_retry(
                            &model,
                            &messages,
                            &system_prompt,
                            &tools_option,
                            cancel_token.clone(),
                            3,
                        )
                        .await
                        .map_err(|e| AgentInterpreterError::Api(e.to_string()))?;

                    let tool_calls = response.extract_tool_calls();
                    let message_id = MessageId::new();
                    let timestamp = current_timestamp();

                    let input = AgentInput::ModelResponse {
                        content: response.content,
                        tool_calls,
                        message_id,
                        timestamp,
                    };

                    let (new_state, outputs) = stepper.step(state, input);
                    state = new_state;
                    pending_outputs.extend(outputs);
                }

                AgentOutput::RequestApproval { tool_call } => {
                    // For now, auto-approve all tools (placeholder for real approval flow)
                    let tool_call_id = ToolCallId::from_string(&tool_call.id);
                    let input = AgentInput::ToolApproved { tool_call_id };
                    let (new_state, outputs) = stepper.step(state, input);
                    state = new_state;
                    pending_outputs.extend(outputs);
                }

                AgentOutput::ExecuteTool { tool_call } => {
                    let tool_call_id = ToolCallId::from_string(&tool_call.id);

                    let result = self
                        .tool_executor
                        .execute_tool_with_cancellation(&tool_call, cancel_token.clone())
                        .await;

                    let message_id = MessageId::new();
                    let timestamp = current_timestamp();

                    let input = match result {
                        Ok(tool_result) => AgentInput::ToolCompleted {
                            tool_call_id,
                            result: tool_result,
                            message_id,
                            timestamp,
                        },
                        Err(error) => AgentInput::ToolFailed {
                            tool_call_id,
                            error,
                            message_id,
                            timestamp,
                        },
                    };

                    let (new_state, outputs) = stepper.step(state, input);
                    state = new_state;
                    pending_outputs.extend(outputs);
                }

                AgentOutput::EmitMessage { message } => {
                    let _ = message_tx.send(message).await;
                }

                AgentOutput::Done { final_message } => {
                    return Ok(final_message);
                }

                AgentOutput::Error { error } => {
                    return Err(AgentInterpreterError::Agent(error));
                }

                AgentOutput::Cancelled => {
                    return Err(AgentInterpreterError::Cancelled);
                }
            }
        }
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, thiserror::Error)]
pub enum AgentInterpreterError {
    #[error("API error: {0}")]
    Api(String),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("Cancelled")]
    Cancelled,
}
