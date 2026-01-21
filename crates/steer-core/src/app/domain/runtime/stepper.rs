use std::collections::HashMap;

use crate::app::conversation::{AssistantContent, Message, MessageData};
use crate::app::domain::types::{MessageId, ToolCallId};
use crate::app::SystemContext;
use crate::config::model::ModelId;
use steer_tools::{ToolCall, ToolError, ToolResult, ToolSchema};

#[derive(Debug, Clone)]
pub enum AgentState {
    AwaitingModel {
        messages: Vec<Message>,
    },
    AwaitingToolApprovals {
        messages: Vec<Message>,
        pending_approvals: Vec<ToolCall>,
        approved: Vec<ToolCall>,
        denied: Vec<ToolCall>,
    },
    AwaitingToolResults {
        messages: Vec<Message>,
        pending_results: HashMap<ToolCallId, ToolCall>,
        completed_results: Vec<(ToolCallId, ToolResult)>,
    },
    Complete {
        final_message: Message,
    },
    Failed {
        error: String,
    },
    Cancelled,
}

#[derive(Debug, Clone)]
pub enum AgentInput {
    ModelResponse {
        content: Vec<AssistantContent>,
        tool_calls: Vec<ToolCall>,
        message_id: MessageId,
        timestamp: u64,
    },
    ModelError {
        error: String,
    },
    ToolApproved {
        tool_call_id: ToolCallId,
    },
    ToolDenied {
        tool_call_id: ToolCallId,
    },
    ToolCompleted {
        tool_call_id: ToolCallId,
        result: ToolResult,
        message_id: MessageId,
        timestamp: u64,
    },
    ToolFailed {
        tool_call_id: ToolCallId,
        error: ToolError,
        message_id: MessageId,
        timestamp: u64,
    },
    Cancel,
}

#[derive(Debug, Clone)]
pub enum AgentOutput {
    CallModel {
        model: ModelId,
        messages: Vec<Message>,
        system_context: Option<SystemContext>,
        tools: Vec<ToolSchema>,
    },
    RequestApproval {
        tool_call: ToolCall,
    },
    ExecuteTool {
        tool_call: ToolCall,
    },
    EmitMessage {
        message: Message,
    },
    Done {
        final_message: Message,
    },
    Error {
        error: String,
    },
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: ModelId,
    pub system_context: Option<SystemContext>,
    pub tools: Vec<ToolSchema>,
}

struct ToolCompletionContext {
    messages: Vec<Message>,
    pending_results: HashMap<ToolCallId, ToolCall>,
    completed_results: Vec<(ToolCallId, ToolResult)>,
    tool_call_id: ToolCallId,
    message_id: MessageId,
    timestamp: u64,
}

pub struct AgentStepper {
    config: AgentConfig,
}

impl AgentStepper {
    pub fn new(config: AgentConfig) -> Self {
        Self { config }
    }

    pub fn initial_state(messages: Vec<Message>) -> AgentState {
        AgentState::AwaitingModel { messages }
    }

    pub fn step(&self, state: AgentState, input: AgentInput) -> (AgentState, Vec<AgentOutput>) {
        match (state, input) {
            (
                AgentState::AwaitingModel { messages },
                AgentInput::ModelResponse {
                    content,
                    tool_calls,
                    message_id,
                    timestamp,
                },
            ) => self.handle_model_response(messages, content, tool_calls, message_id, timestamp),

            (AgentState::AwaitingModel { .. }, AgentInput::ModelError { error }) => (
                AgentState::Failed {
                    error: error.clone(),
                },
                vec![AgentOutput::Error { error }],
            ),

            (
                AgentState::AwaitingToolApprovals {
                    messages,
                    pending_approvals,
                    approved,
                    denied,
                },
                AgentInput::ToolApproved { tool_call_id },
            ) => self.handle_tool_approved(
                messages,
                pending_approvals,
                approved,
                denied,
                tool_call_id,
            ),

            (
                AgentState::AwaitingToolApprovals {
                    messages,
                    pending_approvals,
                    approved,
                    denied,
                },
                AgentInput::ToolDenied { tool_call_id },
            ) => {
                self.handle_tool_denied(messages, pending_approvals, approved, denied, tool_call_id)
            }

            (
                AgentState::AwaitingToolResults {
                    messages,
                    pending_results,
                    completed_results,
                },
                AgentInput::ToolCompleted {
                    tool_call_id,
                    result,
                    message_id,
                    timestamp,
                },
            ) => self.handle_tool_completed(
                ToolCompletionContext {
                    messages,
                    pending_results,
                    completed_results,
                    tool_call_id,
                    message_id,
                    timestamp,
                },
                result,
            ),

            (
                AgentState::AwaitingToolResults {
                    messages,
                    pending_results,
                    completed_results,
                },
                AgentInput::ToolFailed {
                    tool_call_id,
                    error,
                    message_id,
                    timestamp,
                },
            ) => self.handle_tool_failed(
                ToolCompletionContext {
                    messages,
                    pending_results,
                    completed_results,
                    tool_call_id,
                    message_id,
                    timestamp,
                },
                error,
            ),

            (_, AgentInput::Cancel) => (AgentState::Cancelled, vec![AgentOutput::Cancelled]),

            (state, _) => (state, vec![]),
        }
    }

    fn handle_model_response(
        &self,
        mut messages: Vec<Message>,
        content: Vec<AssistantContent>,
        tool_calls: Vec<ToolCall>,
        message_id: MessageId,
        timestamp: u64,
    ) -> (AgentState, Vec<AgentOutput>) {
        let parent_id = messages.last().map(|m| m.id().to_string());

        let assistant_message = Message {
            data: MessageData::Assistant { content },
            timestamp,
            id: message_id.0.clone(),
            parent_message_id: parent_id,
        };

        messages.push(assistant_message.clone());

        let mut outputs = vec![AgentOutput::EmitMessage {
            message: assistant_message.clone(),
        }];

        if tool_calls.is_empty() {
            (
                AgentState::Complete {
                    final_message: assistant_message.clone(),
                },
                vec![
                    AgentOutput::EmitMessage {
                        message: assistant_message.clone(),
                    },
                    AgentOutput::Done {
                        final_message: assistant_message,
                    },
                ],
            )
        } else {
            for tool_call in &tool_calls {
                outputs.push(AgentOutput::RequestApproval {
                    tool_call: tool_call.clone(),
                });
            }

            (
                AgentState::AwaitingToolApprovals {
                    messages,
                    pending_approvals: tool_calls,
                    approved: vec![],
                    denied: vec![],
                },
                outputs,
            )
        }
    }

    fn handle_tool_approved(
        &self,
        messages: Vec<Message>,
        mut pending_approvals: Vec<ToolCall>,
        mut approved: Vec<ToolCall>,
        denied: Vec<ToolCall>,
        tool_call_id: ToolCallId,
    ) -> (AgentState, Vec<AgentOutput>) {
        let mut outputs = vec![];

        if let Some(pos) = pending_approvals
            .iter()
            .position(|tc| tc.id == tool_call_id.0)
        {
            let tool_call = pending_approvals.remove(pos);
            outputs.push(AgentOutput::ExecuteTool {
                tool_call: tool_call.clone(),
            });
            approved.push(tool_call);
        }

        if pending_approvals.is_empty() {
            let mut pending_results = HashMap::new();
            for tc in &approved {
                pending_results.insert(ToolCallId::from_string(&tc.id), tc.clone());
            }

            (
                AgentState::AwaitingToolResults {
                    messages,
                    pending_results,
                    completed_results: vec![],
                },
                outputs,
            )
        } else {
            (
                AgentState::AwaitingToolApprovals {
                    messages,
                    pending_approvals,
                    approved,
                    denied,
                },
                outputs,
            )
        }
    }

    fn handle_tool_denied(
        &self,
        messages: Vec<Message>,
        mut pending_approvals: Vec<ToolCall>,
        approved: Vec<ToolCall>,
        mut denied: Vec<ToolCall>,
        tool_call_id: ToolCallId,
    ) -> (AgentState, Vec<AgentOutput>) {
        if let Some(pos) = pending_approvals
            .iter()
            .position(|tc| tc.id == tool_call_id.0)
        {
            let tool_call = pending_approvals.remove(pos);
            denied.push(tool_call);
        }

        if pending_approvals.is_empty() {
            if approved.is_empty() {
                (
                    AgentState::Failed {
                        error: "All tools denied".to_string(),
                    },
                    vec![AgentOutput::Error {
                        error: "All tools denied".to_string(),
                    }],
                )
            } else {
                let mut pending_results = HashMap::new();
                for tc in &approved {
                    pending_results.insert(ToolCallId::from_string(&tc.id), tc.clone());
                }

                (
                    AgentState::AwaitingToolResults {
                        messages,
                        pending_results,
                        completed_results: vec![],
                    },
                    vec![],
                )
            }
        } else {
            (
                AgentState::AwaitingToolApprovals {
                    messages,
                    pending_approvals,
                    approved,
                    denied,
                },
                vec![],
            )
        }
    }

    fn handle_tool_completed(
        &self,
        mut context: ToolCompletionContext,
        result: ToolResult,
    ) -> (AgentState, Vec<AgentOutput>) {
        let mut outputs = vec![];

        if let Some(tool_call) = context.pending_results.remove(&context.tool_call_id) {
            let parent_id = context.messages.last().map(|m| m.id().to_string());

            let tool_message = Message {
                data: MessageData::Tool {
                    tool_use_id: tool_call.id.clone(),
                    result: result.clone(),
                },
                timestamp: context.timestamp,
                id: context.message_id.0.clone(),
                parent_message_id: parent_id,
            };

            context.messages.push(tool_message.clone());
            outputs.push(AgentOutput::EmitMessage {
                message: tool_message,
            });
            context
                .completed_results
                .push((context.tool_call_id, result));
        }

        if context.pending_results.is_empty() {
            outputs.push(AgentOutput::CallModel {
                model: self.config.model.clone(),
                messages: context.messages.clone(),
                system_context: self.config.system_context.clone(),
                tools: self.config.tools.clone(),
            });

            (
                AgentState::AwaitingModel {
                    messages: context.messages,
                },
                outputs,
            )
        } else {
            (
                AgentState::AwaitingToolResults {
                    messages: context.messages,
                    pending_results: context.pending_results,
                    completed_results: context.completed_results,
                },
                outputs,
            )
        }
    }

    fn handle_tool_failed(
        &self,
        context: ToolCompletionContext,
        error: ToolError,
    ) -> (AgentState, Vec<AgentOutput>) {
        let result = ToolResult::Error(error);
        self.handle_tool_completed(context, result)
    }

    pub fn needs_model_call(&self, state: &AgentState) -> bool {
        matches!(state, AgentState::AwaitingModel { .. })
    }

    pub fn is_terminal(&self, state: &AgentState) -> bool {
        matches!(
            state,
            AgentState::Complete { .. } | AgentState::Failed { .. } | AgentState::Cancelled
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::builtin;

    fn test_config() -> AgentConfig {
        AgentConfig {
            model: builtin::claude_sonnet_4_5(),
            system_context: None,
            tools: vec![],
        }
    }

    #[test]
    fn test_initial_state() {
        let state = AgentStepper::initial_state(vec![]);
        assert!(matches!(state, AgentState::AwaitingModel { .. }));
    }

    #[test]
    fn test_model_response_no_tools_completes() {
        let stepper = AgentStepper::new(test_config());
        let state = AgentState::AwaitingModel { messages: vec![] };

        let (new_state, outputs) = stepper.step(
            state,
            AgentInput::ModelResponse {
                content: vec![],
                tool_calls: vec![],
                message_id: MessageId::new(),
                timestamp: 0,
            },
        );

        assert!(matches!(new_state, AgentState::Complete { .. }));
        assert!(
            outputs
                .iter()
                .any(|o| matches!(o, AgentOutput::Done { .. }))
        );
    }

    #[test]
    fn test_model_response_with_tools_requests_approval() {
        let stepper = AgentStepper::new(test_config());
        let state = AgentState::AwaitingModel { messages: vec![] };

        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "test_tool".to_string(),
            parameters: serde_json::json!({}),
        };

        let (new_state, outputs) = stepper.step(
            state,
            AgentInput::ModelResponse {
                content: vec![],
                tool_calls: vec![tool_call],
                message_id: MessageId::new(),
                timestamp: 0,
            },
        );

        assert!(matches!(
            new_state,
            AgentState::AwaitingToolApprovals { .. }
        ));
        assert!(
            outputs
                .iter()
                .any(|o| matches!(o, AgentOutput::RequestApproval { .. }))
        );
    }

    #[test]
    fn test_cancel_from_any_state() {
        let stepper = AgentStepper::new(test_config());

        let states = vec![
            AgentState::AwaitingModel { messages: vec![] },
            AgentState::AwaitingToolApprovals {
                messages: vec![],
                pending_approvals: vec![],
                approved: vec![],
                denied: vec![],
            },
        ];

        for state in states {
            let (new_state, outputs) = stepper.step(state, AgentInput::Cancel);
            assert!(matches!(new_state, AgentState::Cancelled));
            assert!(outputs.iter().any(|o| matches!(o, AgentOutput::Cancelled)));
        }
    }
}
