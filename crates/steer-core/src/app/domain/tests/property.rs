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
    use proptest::prelude::*;
    use std::collections::HashSet;
    use steer_tools::ToolCall;
    use steer_tools::result::{ExternalResult, ToolResult};

    fn test_model() -> crate::config::model::ModelId {
        builtin::claude_sonnet_4_20250514()
    }

    fn arb_session_id() -> impl Strategy<Value = SessionId> {
        any::<u128>().prop_map(|n| SessionId::from(uuid::Uuid::from_u128(n)))
    }

    fn arb_op_id() -> impl Strategy<Value = OpId> {
        any::<u128>().prop_map(|n| OpId::from(uuid::Uuid::from_u128(n)))
    }

    fn arb_message_id() -> impl Strategy<Value = MessageId> {
        "[a-z]{1,10}".prop_map(|s| MessageId::from_string(&format!("msg_{s}")))
    }

    fn arb_request_id() -> impl Strategy<Value = RequestId> {
        any::<u128>().prop_map(|n| RequestId::from(uuid::Uuid::from_u128(n)))
    }

    fn arb_tool_call_id() -> impl Strategy<Value = ToolCallId> {
        "[a-z]{1,10}".prop_map(|s| ToolCallId::from_string(&format!("tc_{s}")))
    }

    fn arb_non_empty_string() -> impl Strategy<Value = NonEmptyString> {
        "[a-zA-Z0-9 ]{1,50}".prop_filter_map("non-empty", |s| NonEmptyString::new(s))
    }

    fn arb_tool_name() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("bash".to_string()),
            Just("read_file".to_string()),
            Just("write_file".to_string()),
            Just("search".to_string()),
        ]
    }

    fn arb_tool_call() -> impl Strategy<Value = ToolCall> {
        (arb_tool_call_id(), arb_tool_name()).prop_map(|(id, name)| ToolCall {
            id: id.0,
            name,
            parameters: serde_json::json!({}),
        })
    }

    fn arb_user_input_action(session_id: SessionId) -> impl Strategy<Value = Action> {
        (
            arb_op_id(),
            arb_message_id(),
            arb_non_empty_string(),
            0u64..1000000u64,
        )
            .prop_map(
                move |(op_id, message_id, text, timestamp)| Action::UserInput {
                    session_id,
                    text,
                    op_id,
                    message_id,
                    timestamp,
                    model: test_model(),
                },
            )
    }

    fn arb_cancel_action(session_id: SessionId) -> impl Strategy<Value = Action> {
        prop_oneof![
            Just(Action::Cancel {
                session_id,
                op_id: None,
            }),
            arb_op_id().prop_map(move |op_id| Action::Cancel {
                session_id,
                op_id: Some(op_id),
            }),
        ]
    }

    fn arb_tool_approval_requested(session_id: SessionId) -> impl Strategy<Value = Action> {
        (arb_request_id(), arb_tool_call()).prop_map(move |(request_id, tool_call)| {
            Action::ToolApprovalRequested {
                session_id,
                request_id,
                tool_call,
            }
        })
    }

    fn arb_tool_approval_decided(session_id: SessionId) -> impl Strategy<Value = Action> {
        (
            arb_request_id(),
            any::<bool>(),
            prop::option::of(arb_tool_name()),
        )
            .prop_map(move |(request_id, approved, remember_tool)| {
                Action::ToolApprovalDecided {
                    session_id,
                    request_id,
                    decision: if approved {
                        ApprovalDecision::Approved
                    } else {
                        ApprovalDecision::Denied
                    },
                    remember: remember_tool.map(ApprovalMemory::Tool),
                }
            })
    }

    fn count_effects_by_type(effects: &[Effect]) -> (usize, usize, usize, usize) {
        let mut emit_events = 0;
        let mut call_models = 0;
        let mut request_approvals = 0;
        let mut execute_tools = 0;

        for effect in effects {
            match effect {
                Effect::EmitEvent { .. } => emit_events += 1,
                Effect::CallModel { .. } => call_models += 1,
                Effect::RequestUserApproval { .. } => request_approvals += 1,
                Effect::ExecuteTool { .. } => execute_tools += 1,
                _ => {}
            }
        }

        (emit_events, call_models, request_approvals, execute_tools)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_reducer_is_deterministic(
            session_id in arb_session_id(),
            op_id in arb_op_id(),
            message_id in arb_message_id(),
            text in arb_non_empty_string(),
            timestamp in 0u64..1000000u64,
        ) {
            let action = Action::UserInput {
                session_id,
                text: text.clone(),
                op_id,
                message_id: message_id.clone(),
                timestamp,
                model: test_model(),
            };

            let mut state1 = AppState::new(session_id, test_model());
            let effects1 = reduce(&mut state1, action.clone());

            let mut state2 = AppState::new(session_id, test_model());
            let effects2 = reduce(&mut state2, action);

            prop_assert_eq!(
                state1.conversation.messages.len(),
                state2.conversation.messages.len(),
                "Message counts should be equal"
            );
            prop_assert_eq!(
                state1.current_operation.is_some(),
                state2.current_operation.is_some(),
                "Operation presence should be equal"
            );
            prop_assert_eq!(
                effects1.len(),
                effects2.len(),
                "Effect counts should be equal"
            );
        }

        #[test]
        fn prop_user_input_always_starts_operation(
            session_id in arb_session_id(),
            action in arb_user_input_action(SessionId::from(uuid::Uuid::from_u128(12345))).prop_map(|a| {
                if let Action::UserInput { text, op_id, message_id, timestamp, .. } = a {
                    Action::UserInput {
                        session_id: SessionId::from(uuid::Uuid::from_u128(12345)),
                        text,
                        op_id,
                        message_id,
                        timestamp,
                        model: test_model(),
                    }
                } else {
                    unreachable!()
                }
            }),
        ) {
            let session_id = SessionId::from(uuid::Uuid::from_u128(12345));
            let mut state = AppState::new(session_id, test_model());

            let effects = reduce(&mut state, action);

            prop_assert!(state.current_operation.is_some(), "Should have an operation");
            prop_assert!(
                effects.iter().any(|e| matches!(e, Effect::CallModel { .. })),
                "Should emit CallModel effect"
            );
            prop_assert_eq!(state.conversation.messages.len(), 1, "Should add one message");
        }

        #[test]
        fn prop_cancel_clears_operation_and_records_cancelled(
            session_id in arb_session_id(),
            op_id in arb_op_id(),
        ) {
            let mut state = AppState::new(session_id, test_model());
            state.current_operation = Some(OperationState {
                op_id,
                kind: OperationKind::AgentLoop,
                pending_tool_calls: HashSet::new(),
            });

            let _ = reduce(&mut state, Action::Cancel {
                session_id,
                op_id: None,
            });

            prop_assert!(state.current_operation.is_none(), "Operation should be cleared");
            prop_assert!(state.cancelled_ops.contains(&op_id), "Op should be recorded as cancelled");
        }

        #[test]
        fn prop_cancelled_ops_limit_is_enforced(
            session_id in arb_session_id(),
            op_ids in prop::collection::vec(arb_op_id(), 110..120),
        ) {
            let mut state = AppState::new(session_id, test_model());

            for op_id in op_ids {
                state.current_operation = Some(OperationState {
                    op_id,
                    kind: OperationKind::AgentLoop,
                    pending_tool_calls: HashSet::new(),
                });
                let _ = reduce(&mut state, Action::Cancel {
                    session_id,
                    op_id: None,
                });
            }

            prop_assert!(
                state.cancelled_ops.len() <= 100,
                "Cancelled ops should be bounded at 100, got {}",
                state.cancelled_ops.len()
            );
        }

        #[test]
        fn prop_late_tool_result_is_ignored_for_cancelled_op(
            session_id in arb_session_id(),
            op_id in arb_op_id(),
            tool_call_id in arb_tool_call_id(),
        ) {
            let mut state = AppState::new(session_id, test_model());
            state.current_operation = Some(OperationState {
                op_id,
                kind: OperationKind::AgentLoop,
                pending_tool_calls: [tool_call_id.clone()].into_iter().collect(),
            });

            let _ = reduce(&mut state, Action::Cancel {
                session_id,
                op_id: None,
            });

            state.current_operation = Some(OperationState {
                op_id,
                kind: OperationKind::AgentLoop,
                pending_tool_calls: HashSet::new(),
            });

            let effects = reduce(&mut state, Action::ToolResult {
                session_id,
                tool_call_id,
                tool_name: "test".to_string(),
                result: Ok(ToolResult::External(ExternalResult {
                    tool_name: "test".to_string(),
                    payload: "done".to_string(),
                })),
            });

            prop_assert!(effects.is_empty(), "Late result should produce no effects");
        }

        #[test]
        fn prop_pre_approved_tool_skips_approval(
            session_id in arb_session_id(),
            op_id in arb_op_id(),
            tool_name in arb_tool_name(),
        ) {
            let mut state = AppState::new(session_id, test_model());
            state.approved_tools.insert(tool_name.clone());
            state.current_operation = Some(OperationState {
                op_id,
                kind: OperationKind::AgentLoop,
                pending_tool_calls: HashSet::new(),
            });

            let tool_call = ToolCall {
                id: "tc_1".to_string(),
                name: tool_name,
                parameters: serde_json::json!({}),
            };

            let effects = reduce(&mut state, Action::ToolApprovalRequested {
                session_id,
                request_id: RequestId::new(),
                tool_call,
            });

            prop_assert!(state.pending_approval.is_none(), "Should not have pending approval");
            prop_assert!(
                effects.iter().any(|e| matches!(e, Effect::ExecuteTool { .. })),
                "Should execute tool directly"
            );
        }

        #[test]
        fn prop_model_response_without_tools_completes_operation(
            session_id in arb_session_id(),
            op_id in arb_op_id(),
            message_id in arb_message_id(),
            text in "[a-zA-Z ]{1,100}",
            timestamp in 0u64..1000000u64,
        ) {
            let mut state = AppState::new(session_id, test_model());
            state.current_operation = Some(OperationState {
                op_id,
                kind: OperationKind::AgentLoop,
                pending_tool_calls: HashSet::new(),
            });

            let content = vec![AssistantContent::Text { text }];
            let effects = reduce(&mut state, Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id,
                content,
                timestamp,
            });

            prop_assert!(state.current_operation.is_none(), "Operation should complete");
            prop_assert!(
                effects.iter().any(|e| matches!(e, Effect::EmitEvent {
                    event: SessionEvent::OperationCompleted { .. },
                    ..
                })),
                "Should emit OperationCompleted"
            );
        }

        #[test]
        fn prop_approval_queuing_works_correctly(
            session_id in arb_session_id(),
            op_id in arb_op_id(),
            tool_calls in prop::collection::vec(arb_tool_call(), 2..5),
        ) {
            let mut state = AppState::new(session_id, test_model());
            state.current_operation = Some(OperationState {
                op_id,
                kind: OperationKind::AgentLoop,
                pending_tool_calls: HashSet::new(),
            });

            for tool_call in &tool_calls {
                let _ = reduce(&mut state, Action::ToolApprovalRequested {
                    session_id,
                    request_id: RequestId::new(),
                    tool_call: tool_call.clone(),
                });
            }

            prop_assert!(state.pending_approval.is_some(), "Should have pending approval");
            prop_assert_eq!(
                state.approval_queue.len(),
                tool_calls.len() - 1,
                "Should queue remaining approvals"
            );
        }
    }
}
