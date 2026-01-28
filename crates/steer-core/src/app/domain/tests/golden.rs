#[cfg(test)]
mod tests {
    use crate::app::conversation::AssistantContent;
    use crate::app::domain::action::{Action, ApprovalDecision, ApprovalMemory};
    use crate::app::domain::effect::Effect;
    use crate::app::domain::event::SessionEvent;
    use crate::app::domain::reduce::reduce;
    use crate::app::domain::state::{AppState, OperationKind, OperationState};
    use crate::app::domain::types::{
        MessageId, NonEmptyString, OpId, RequestId, SessionId, ToolCallId,
    };
    use crate::config::model::builtin;
    use serde::{Deserialize, Serialize};
    use std::collections::HashSet;
    use steer_tools::result::{ExternalResult, ToolResult};
    use steer_tools::{ToolCall, ToolError};

    fn test_state() -> AppState {
        AppState::new(SessionId::new())
    }

    fn deterministic_session_id() -> SessionId {
        SessionId::from(uuid::Uuid::from_u128(
            0x12345678_1234_1234_1234_123456789abc,
        ))
    }

    fn deterministic_op_id(n: u128) -> OpId {
        OpId::from(uuid::Uuid::from_u128(n))
    }

    fn deterministic_message_id(s: &str) -> MessageId {
        MessageId::from_string(s)
    }

    fn deterministic_request_id(n: u128) -> RequestId {
        RequestId::from(uuid::Uuid::from_u128(n))
    }

    fn deterministic_tool_call_id(s: &str) -> ToolCallId {
        ToolCallId::from_string(s)
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct GoldenTestCase {
        name: String,
        actions: Vec<ActionSnapshot>,
        expected_effects: Vec<EffectSnapshot>,
        expected_state: StateSnapshot,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(tag = "type")]
    enum ActionSnapshot {
        UserInput {
            text: String,
            op_id: u128,
            message_id: String,
            timestamp: u64,
        },
        ModelResponseComplete {
            op_id: u128,
            message_id: String,
            content: Vec<ContentSnapshot>,
            timestamp: u64,
        },
        ToolApprovalRequested {
            request_id: u128,
            tool_name: String,
            tool_id: String,
        },
        ToolApprovalDecided {
            request_id: u128,
            approved: bool,
            remember_tool: Option<String>,
        },
        ToolResult {
            tool_call_id: String,
            success: bool,
            payload: String,
        },
        Cancel {
            op_id: Option<u128>,
        },
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(tag = "type")]
    enum ContentSnapshot {
        Text { text: String },
        ToolCall { id: String, name: String },
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(tag = "type")]
    enum EffectSnapshot {
        EmitEvent { event_type: String },
        CallModel,
        RequestUserApproval { tool_name: String },
        ExecuteTool { tool_name: String },
        CancelOperation,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct StateSnapshot {
        message_count: usize,
        has_operation: bool,
        operation_kind: Option<String>,
        pending_approval: Option<String>,
        approved_tools: Vec<String>,
        cancelled_ops_count: usize,
    }

    fn snapshot_effect(effect: &Effect) -> EffectSnapshot {
        match effect {
            Effect::EmitEvent { event, .. } => EffectSnapshot::EmitEvent {
                event_type: match event {
                    SessionEvent::AssistantMessageAdded { .. } => {
                        "AssistantMessageAdded".to_string()
                    }
                    SessionEvent::UserMessageAdded { .. } => "UserMessageAdded".to_string(),
                    SessionEvent::ToolMessageAdded { .. } => "ToolMessageAdded".to_string(),
                    SessionEvent::OperationStarted { .. } => "OperationStarted".to_string(),
                    SessionEvent::OperationCompleted { .. } => "OperationCompleted".to_string(),
                    SessionEvent::OperationCancelled { .. } => "OperationCancelled".to_string(),
                    SessionEvent::ApprovalRequested { .. } => "ApprovalRequested".to_string(),
                    SessionEvent::ApprovalDecided { .. } => "ApprovalDecided".to_string(),
                    SessionEvent::ToolCallStarted { .. } => "ToolCallStarted".to_string(),
                    SessionEvent::ToolCallCompleted { .. } => "ToolCallCompleted".to_string(),
                    SessionEvent::ToolCallFailed { .. } => "ToolCallFailed".to_string(),
                    SessionEvent::Error { .. } => "Error".to_string(),
                    SessionEvent::SessionCreated { .. } => "SessionCreated".to_string(),
                    SessionEvent::SessionConfigUpdated { .. } => {
                        "SessionConfigUpdated".to_string()
                    }
                    SessionEvent::MessageUpdated { .. } => "MessageUpdated".to_string(),
                    SessionEvent::WorkspaceChanged => "WorkspaceChanged".to_string(),
                    SessionEvent::ConversationCompacted { .. } => {
                        "ConversationCompacted".to_string()
                    }
                    SessionEvent::CompactResult { .. } => "CompactResult".to_string(),
                    SessionEvent::McpServerStateChanged { .. } => {
                        "McpServerStateChanged".to_string()
                    }
                    SessionEvent::QueueUpdated { .. } => "QueueUpdated".to_string(),
                },
            },
            Effect::CallModel { .. } => EffectSnapshot::CallModel,
            Effect::RequestUserApproval { tool_call, .. } => EffectSnapshot::RequestUserApproval {
                tool_name: tool_call.name.clone(),
            },
            Effect::ExecuteTool { tool_call, .. } => EffectSnapshot::ExecuteTool {
                tool_name: tool_call.name.clone(),
            },
            Effect::CancelOperation { .. } => EffectSnapshot::CancelOperation,
            _ => EffectSnapshot::EmitEvent {
                event_type: "Unknown".to_string(),
            },
        }
    }

    fn snapshot_state(state: &AppState) -> StateSnapshot {
        StateSnapshot {
            message_count: state.message_graph.messages.len(),
            has_operation: state.current_operation.is_some(),
            operation_kind: state.current_operation.as_ref().map(|op| match op.kind {
                OperationKind::AgentLoop => "AgentLoop".to_string(),
                OperationKind::Compact => "Compact".to_string(),
                OperationKind::DirectBash { .. } => "DirectBash".to_string(),
            }),
            pending_approval: state
                .pending_approval
                .as_ref()
                .map(|p| p.tool_call.name.clone()),
            approved_tools: state.approved_tools.iter().cloned().collect(),
            cancelled_ops_count: state.cancelled_ops.len(),
        }
    }

    fn to_action(snapshot: &ActionSnapshot, session_id: SessionId) -> Action {
        match snapshot {
            ActionSnapshot::UserInput {
                text,
                op_id,
                message_id,
                timestamp,
            } => Action::UserInput {
                session_id,
                text: NonEmptyString::new(text.clone()).unwrap(),
                op_id: deterministic_op_id(*op_id),
                message_id: deterministic_message_id(message_id),
                timestamp: *timestamp,
                model: builtin::claude_sonnet_4_5(),
            },
            ActionSnapshot::ModelResponseComplete {
                op_id,
                message_id,
                content,
                timestamp,
            } => {
                let content: Vec<AssistantContent> = content
                    .iter()
                    .map(|c| match c {
                        ContentSnapshot::Text { text } => {
                            AssistantContent::Text { text: text.clone() }
                        }
                        ContentSnapshot::ToolCall { id, name } => AssistantContent::ToolCall {
                            tool_call: ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                parameters: serde_json::json!({}),
                            },
                            thought_signature: None,
                        },
                    })
                    .collect();
                Action::ModelResponseComplete {
                    session_id,
                    op_id: deterministic_op_id(*op_id),
                    message_id: deterministic_message_id(message_id),
                    content,
                    timestamp: *timestamp,
                }
            }
            ActionSnapshot::ToolApprovalRequested {
                request_id,
                tool_name,
                tool_id,
            } => Action::ToolApprovalRequested {
                session_id,
                request_id: deterministic_request_id(*request_id),
                tool_call: ToolCall {
                    id: tool_id.clone(),
                    name: tool_name.clone(),
                    parameters: serde_json::json!({}),
                },
            },
            ActionSnapshot::ToolApprovalDecided {
                request_id,
                approved,
                remember_tool,
            } => Action::ToolApprovalDecided {
                session_id,
                request_id: deterministic_request_id(*request_id),
                decision: if *approved {
                    ApprovalDecision::Approved
                } else {
                    ApprovalDecision::Denied
                },
                remember: remember_tool.clone().map(ApprovalMemory::Tool),
            },
            ActionSnapshot::ToolResult {
                tool_call_id,
                success,
                payload,
            } => Action::ToolResult {
                session_id,
                tool_call_id: deterministic_tool_call_id(tool_call_id),
                tool_name: "test".to_string(),
                result: if *success {
                    Ok(ToolResult::External(ExternalResult {
                        tool_name: "test".to_string(),
                        payload: payload.clone(),
                    }))
                } else {
                    Err(ToolError::execution("test", payload.clone()))
                },
            },
            ActionSnapshot::Cancel { op_id } => Action::Cancel {
                session_id,
                op_id: op_id.map(deterministic_op_id),
            },
        }
    }

    fn run_golden_test(test_case: &GoldenTestCase) {
        let session_id = deterministic_session_id();
        let mut state = AppState::new(session_id);

        let mut all_effects = Vec::new();
        for action_snapshot in &test_case.actions {
            let action = to_action(action_snapshot, session_id);
            let effects = reduce(&mut state, action);
            all_effects.extend(effects);
        }

        let effect_snapshots: Vec<EffectSnapshot> =
            all_effects.iter().map(snapshot_effect).collect();
        let state_snapshot = snapshot_state(&state);

        assert_eq!(
            effect_snapshots, test_case.expected_effects,
            "Effects mismatch for test '{}'",
            test_case.name
        );
        assert_eq!(
            state_snapshot, test_case.expected_state,
            "State mismatch for test '{}'",
            test_case.name
        );
    }

    #[test]
    fn golden_user_input_starts_agent_loop() {
        let test_case = GoldenTestCase {
            name: "user_input_starts_agent_loop".to_string(),
            actions: vec![ActionSnapshot::UserInput {
                text: "Hello, world!".to_string(),
                op_id: 1,
                message_id: "msg_1".to_string(),
                timestamp: 1000,
            }],
            expected_effects: vec![
                EffectSnapshot::EmitEvent {
                    event_type: "UserMessageAdded".to_string(),
                },
                EffectSnapshot::EmitEvent {
                    event_type: "OperationStarted".to_string(),
                },
                EffectSnapshot::CallModel,
            ],
            expected_state: StateSnapshot {
                message_count: 1,
                has_operation: true,
                operation_kind: Some("AgentLoop".to_string()),
                pending_approval: None,
                approved_tools: vec![],
                cancelled_ops_count: 0,
            },
        };
        run_golden_test(&test_case);
    }

    #[test]
    fn golden_model_response_no_tools_completes() {
        let test_case = GoldenTestCase {
            name: "model_response_no_tools_completes".to_string(),
            actions: vec![
                ActionSnapshot::UserInput {
                    text: "Hello".to_string(),
                    op_id: 1,
                    message_id: "msg_1".to_string(),
                    timestamp: 1000,
                },
                ActionSnapshot::ModelResponseComplete {
                    op_id: 1,
                    message_id: "msg_2".to_string(),
                    content: vec![ContentSnapshot::Text {
                        text: "Hi there!".to_string(),
                    }],
                    timestamp: 1001,
                },
            ],
            expected_effects: vec![
                EffectSnapshot::EmitEvent {
                    event_type: "UserMessageAdded".to_string(),
                },
                EffectSnapshot::EmitEvent {
                    event_type: "OperationStarted".to_string(),
                },
                EffectSnapshot::CallModel,
                EffectSnapshot::EmitEvent {
                    event_type: "AssistantMessageAdded".to_string(),
                },
                EffectSnapshot::EmitEvent {
                    event_type: "OperationCompleted".to_string(),
                },
            ],
            expected_state: StateSnapshot {
                message_count: 2,
                has_operation: false,
                operation_kind: None,
                pending_approval: None,
                approved_tools: vec![],
                cancelled_ops_count: 0,
            },
        };
        run_golden_test(&test_case);
    }

    #[test]
    fn golden_tool_approval_flow() {
        let session_id = deterministic_session_id();
        let mut state = AppState::new(session_id);
        let op_id = deterministic_op_id(1);

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            parameters: serde_json::json!({"command": "ls"}),
        };

        let effects = reduce(
            &mut state,
            Action::ToolApprovalRequested {
                session_id,
                request_id: deterministic_request_id(100),
                tool_call,
            },
        );

        assert!(state.pending_approval.is_some());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RequestUserApproval { .. }))
        );

        let effects = reduce(
            &mut state,
            Action::ToolApprovalDecided {
                session_id,
                request_id: deterministic_request_id(100),
                decision: ApprovalDecision::Approved,
                remember: Some(ApprovalMemory::Tool("bash".to_string())),
            },
        );

        assert!(state.pending_approval.is_none());
        assert!(state.approved_tools.contains("bash"));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExecuteTool { .. }))
        );
    }

    #[test]
    fn golden_full_conversation_flow() {
        let session_id = deterministic_session_id();
        let mut state = AppState::new(session_id);

        let op_id = deterministic_op_id(1);
        let _ = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("List files").unwrap(),
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: builtin::claude_sonnet_4_5(),
            },
        );

        assert_eq!(state.message_graph.messages.len(), 1);
        assert!(state.current_operation.is_some());

        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            parameters: serde_json::json!({"command": "ls"}),
        };
        let _ = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id: deterministic_message_id("msg_2"),
                content: vec![
                    AssistantContent::Text {
                        text: "I'll list the files.".to_string(),
                    },
                    AssistantContent::ToolCall {
                        tool_call: tool_call.clone(),
                        thought_signature: None,
                    },
                ],
                timestamp: 1001,
            },
        );

        assert_eq!(state.message_graph.messages.len(), 2);
        assert!(state.pending_approval.is_some());

        state.approved_tools.insert("bash".to_string());

        let request_id = state.pending_approval.as_ref().unwrap().request_id;
        let _ = reduce(
            &mut state,
            Action::ToolApprovalDecided {
                session_id,
                request_id,
                decision: ApprovalDecision::Approved,
                remember: None,
            },
        );

        assert!(state.pending_approval.is_none());

        let _ = reduce(
            &mut state,
            Action::ToolExecutionStarted {
                session_id,
                tool_call_id: deterministic_tool_call_id("tc_1"),
                tool_name: "bash".to_string(),
                tool_parameters: serde_json::json!({"command": "ls"}),
            },
        );

        let _ = reduce(
            &mut state,
            Action::ToolResult {
                session_id,
                tool_call_id: deterministic_tool_call_id("tc_1"),
                tool_name: "bash".to_string(),
                result: Ok(ToolResult::External(ExternalResult {
                    tool_name: "bash".to_string(),
                    payload: "file1.txt\nfile2.txt".to_string(),
                })),
            },
        );

        assert_eq!(state.message_graph.messages.len(), 3);

        let _ = reduce(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id: deterministic_message_id("msg_4"),
                content: vec![AssistantContent::Text {
                    text: "Found 2 files.".to_string(),
                }],
                timestamp: 1003,
            },
        );

        assert_eq!(state.message_graph.messages.len(), 4);
        assert!(state.current_operation.is_none());
    }

    #[test]
    fn golden_cancellation_clears_state() {
        let session_id = deterministic_session_id();
        let mut state = AppState::new(session_id);
        let op_id = deterministic_op_id(1);

        let _ = reduce(
            &mut state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("Hello").unwrap(),
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: builtin::claude_sonnet_4_5(),
            },
        );

        assert!(state.current_operation.is_some());

        let effects = reduce(
            &mut state,
            Action::Cancel {
                session_id,
                op_id: None,
            },
        );

        assert!(state.current_operation.is_none());
        assert!(state.cancelled_ops.contains(&op_id));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::CancelOperation { .. }))
        );
    }

    #[test]
    fn golden_late_result_ignored_after_cancel() {
        let session_id = deterministic_session_id();
        let mut state = AppState::new(session_id);
        let op_id = deterministic_op_id(1);
        let tool_call_id = deterministic_tool_call_id("tc_1");

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: [tool_call_id.clone()].into_iter().collect(),
        });

        let _ = reduce(
            &mut state,
            Action::Cancel {
                session_id,
                op_id: None,
            },
        );

        assert!(state.cancelled_ops.contains(&op_id));

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        let effects = reduce(
            &mut state,
            Action::ToolResult {
                session_id,
                tool_call_id,
                tool_name: "test".to_string(),
                result: Ok(ToolResult::External(ExternalResult {
                    tool_name: "test".to_string(),
                    payload: "done".to_string(),
                })),
            },
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn golden_pre_approved_tool_executes_immediately() {
        let session_id = deterministic_session_id();
        let mut state = AppState::new(session_id);
        let op_id = deterministic_op_id(1);

        state.approved_tools.insert("bash".to_string());
        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            parameters: serde_json::json!({"command": "ls"}),
        };

        let effects = reduce(
            &mut state,
            Action::ToolApprovalRequested {
                session_id,
                request_id: deterministic_request_id(100),
                tool_call,
            },
        );

        assert!(state.pending_approval.is_none());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ExecuteTool { .. }))
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::RequestUserApproval { .. }))
        );
    }

    #[test]
    fn golden_approval_queuing() {
        let session_id = deterministic_session_id();
        let mut state = AppState::new(session_id);
        let op_id = deterministic_op_id(1);

        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        let tool_call_1 = ToolCall {
            id: "tc_1".to_string(),
            name: "tool_1".to_string(),
            parameters: serde_json::json!({}),
        };
        let tool_call_2 = ToolCall {
            id: "tc_2".to_string(),
            name: "tool_2".to_string(),
            parameters: serde_json::json!({}),
        };

        let _ = reduce(
            &mut state,
            Action::ToolApprovalRequested {
                session_id,
                request_id: deterministic_request_id(100),
                tool_call: tool_call_1,
            },
        );

        assert!(state.pending_approval.is_some());
        assert_eq!(state.approval_queue.len(), 0);

        let _ = reduce(
            &mut state,
            Action::ToolApprovalRequested {
                session_id,
                request_id: deterministic_request_id(101),
                tool_call: tool_call_2,
            },
        );

        assert!(state.pending_approval.is_some());
        assert_eq!(state.approval_queue.len(), 1);

        let _ = reduce(
            &mut state,
            Action::ToolApprovalDecided {
                session_id,
                request_id: deterministic_request_id(100),
                decision: ApprovalDecision::Approved,
                remember: None,
            },
        );

        assert!(state.pending_approval.is_some());
        assert_eq!(
            state.pending_approval.as_ref().unwrap().tool_call.name,
            "tool_2"
        );
        assert_eq!(state.approval_queue.len(), 0);
    }
}
