#[cfg(test)]
mod tests {
    use crate::app::conversation::AssistantContent;
    use crate::app::domain::action::{Action, ApprovalDecision, ApprovalMemory};
    use crate::app::domain::effect::Effect;
    use crate::app::domain::event::SessionEvent;
    use crate::app::domain::reduce::{apply_event_to_state, reduce};
    use crate::app::domain::state::{AppState, OperationKind, OperationState};
    use crate::app::domain::types::{
        MessageId, NonEmptyString, OpId, RequestId, SessionId, ToolCallId,
    };
    use crate::config::model::builtin;
    use std::collections::HashSet;
    use steer_tools::ToolCall;

    fn test_model() -> crate::config::model::ModelId {
        builtin::claude_sonnet_4_20250514()
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

    fn collect_events(effects: &[Effect]) -> Vec<SessionEvent> {
        effects
            .iter()
            .filter_map(|e| {
                if let Effect::EmitEvent { event, .. } = e {
                    Some(event.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn replay_user_input_produces_same_state() {
        let session_id = deterministic_session_id();
        let op_id = deterministic_op_id(1);
        let message_id = deterministic_message_id("msg_1");

        let mut live_state = AppState::new(session_id, test_model());
        let effects = reduce(
            &mut live_state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("Hello world").unwrap(),
                op_id,
                message_id,
                timestamp: 1000,
                model: test_model(),
            },
        );

        let events = collect_events(&effects);

        let mut replayed_state = AppState::new(session_id, test_model());
        for event in &events {
            apply_event_to_state(&mut replayed_state, event);
        }

        assert_eq!(
            live_state.conversation.messages.len(),
            replayed_state.conversation.messages.len(),
            "Message count should match after replay"
        );

        assert_eq!(
            live_state.event_sequence + events.len() as u64,
            replayed_state.event_sequence,
            "Event sequence should be updated"
        );
    }

    #[test]
    fn replay_full_conversation_produces_same_state() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id, test_model());
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);
        let effects = reduce(
            &mut live_state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("List files").unwrap(),
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: test_model(),
            },
        );
        all_events.extend(collect_events(&effects));

        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            parameters: serde_json::json!({"command": "ls"}),
        };
        let effects = reduce(
            &mut live_state,
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
                    },
                ],
                timestamp: 1001,
            },
        );
        all_events.extend(collect_events(&effects));

        let request_id = live_state.pending_approval.as_ref().unwrap().request_id;
        let effects = reduce(
            &mut live_state,
            Action::ToolApprovalDecided {
                session_id,
                request_id,
                decision: ApprovalDecision::Approved,
                remember: Some(ApprovalMemory::Tool("bash".to_string())),
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id, test_model());
        for event in &all_events {
            apply_event_to_state(&mut replayed_state, event);
        }

        assert_eq!(
            live_state.conversation.messages.len(),
            replayed_state.conversation.messages.len(),
            "Message count should match"
        );

        assert!(
            replayed_state.approved_tools.contains("bash"),
            "Approved tool should be replayed"
        );

        assert!(
            replayed_state.pending_approval.is_none(),
            "Pending approval should be cleared after approval decision"
        );
    }

    #[test]
    fn replay_cancellation_produces_same_state() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id, test_model());
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);
        let effects = reduce(
            &mut live_state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("Hello").unwrap(),
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: test_model(),
            },
        );
        all_events.extend(collect_events(&effects));

        let effects = reduce(
            &mut live_state,
            Action::Cancel {
                session_id,
                op_id: None,
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id, test_model());
        for event in &all_events {
            apply_event_to_state(&mut replayed_state, event);
        }

        assert!(
            replayed_state.current_operation.is_none(),
            "Operation should be cleared after replay"
        );

        assert!(
            replayed_state.cancelled_ops.contains(&op_id),
            "Cancelled op should be recorded after replay"
        );
    }

    #[test]
    fn replay_model_change_produces_same_state() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id, test_model());

        let new_model = builtin::claude_opus_4_20250514();
        live_state.current_model = new_model.clone();

        let event = SessionEvent::ModelChanged {
            model: new_model.clone(),
        };

        let mut replayed_state = AppState::new(session_id, test_model());
        apply_event_to_state(&mut replayed_state, &event);

        assert_eq!(
            replayed_state.current_model.1, new_model.1,
            "Model should be updated after replay"
        );
    }

    #[test]
    fn replay_multiple_approvals_produces_same_state() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id, test_model());
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);
        live_state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        for (i, tool_name) in ["bash", "read_file", "write_file"].iter().enumerate() {
            let request_id = deterministic_request_id(100 + i as u128);
            let tool_call = ToolCall {
                id: format!("tc_{i}"),
                name: tool_name.to_string(),
                parameters: serde_json::json!({}),
            };

            let effects = reduce(
                &mut live_state,
                Action::ToolApprovalRequested {
                    session_id,
                    request_id,
                    tool_call,
                },
            );
            all_events.extend(collect_events(&effects));
        }

        let pending_request_id = live_state.pending_approval.as_ref().unwrap().request_id;
        let effects = reduce(
            &mut live_state,
            Action::ToolApprovalDecided {
                session_id,
                request_id: pending_request_id,
                decision: ApprovalDecision::Approved,
                remember: Some(ApprovalMemory::Tool("bash".to_string())),
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id, test_model());
        replayed_state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        for event in &all_events {
            apply_event_to_state(&mut replayed_state, event);
        }

        assert!(
            replayed_state.approved_tools.contains("bash"),
            "bash should be approved after replay"
        );
    }

    #[test]
    fn hydrate_action_produces_same_state_as_replay() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id, test_model());
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);
        let effects = reduce(
            &mut live_state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("Hello").unwrap(),
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: test_model(),
            },
        );
        all_events.extend(collect_events(&effects));

        let effects = reduce(
            &mut live_state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id: deterministic_message_id("msg_2"),
                content: vec![AssistantContent::Text {
                    text: "Hello!".to_string(),
                }],
                timestamp: 1001,
            },
        );
        all_events.extend(collect_events(&effects));

        let mut hydrated_state = AppState::new(session_id, test_model());
        let _ = reduce(
            &mut hydrated_state,
            Action::Hydrate {
                session_id,
                events: all_events.clone(),
                starting_sequence: all_events.len() as u64,
            },
        );

        let mut manually_replayed = AppState::new(session_id, test_model());
        for event in &all_events {
            apply_event_to_state(&mut manually_replayed, event);
        }

        assert_eq!(
            hydrated_state.conversation.messages.len(),
            manually_replayed.conversation.messages.len(),
            "Message count should match"
        );

        assert_eq!(
            hydrated_state.event_sequence,
            all_events.len() as u64,
            "Event sequence should be set correctly"
        );
    }

    #[test]
    fn replay_preserves_message_order() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id, test_model());
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);

        let effects = reduce(
            &mut live_state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("First message").unwrap(),
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: test_model(),
            },
        );
        all_events.extend(collect_events(&effects));

        let effects = reduce(
            &mut live_state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id: deterministic_message_id("msg_2"),
                content: vec![AssistantContent::Text {
                    text: "First response".to_string(),
                }],
                timestamp: 1001,
            },
        );
        all_events.extend(collect_events(&effects));

        let op_id_2 = deterministic_op_id(2);
        let effects = reduce(
            &mut live_state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("Second message").unwrap(),
                op_id: op_id_2,
                message_id: deterministic_message_id("msg_3"),
                timestamp: 2000,
                model: test_model(),
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id, test_model());
        for event in &all_events {
            apply_event_to_state(&mut replayed_state, event);
        }

        assert_eq!(
            live_state.conversation.messages.len(),
            replayed_state.conversation.messages.len(),
            "Should have same number of messages"
        );

        for (i, (live_msg, replayed_msg)) in live_state
            .conversation
            .messages
            .iter()
            .zip(replayed_state.conversation.messages.iter())
            .enumerate()
        {
            assert_eq!(
                live_msg.id(),
                replayed_msg.id(),
                "Message {} ID should match",
                i
            );
        }
    }

    #[test]
    fn replay_bash_pattern_approval() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id, test_model());
        let op_id = deterministic_op_id(1);

        live_state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });

        let request_id = deterministic_request_id(100);
        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            parameters: serde_json::json!({"command": "ls -la"}),
        };

        let effects = reduce(
            &mut live_state,
            Action::ToolApprovalRequested {
                session_id,
                request_id,
                tool_call,
            },
        );
        let mut all_events = collect_events(&effects);

        let effects = reduce(
            &mut live_state,
            Action::ToolApprovalDecided {
                session_id,
                request_id,
                decision: ApprovalDecision::Approved,
                remember: Some(ApprovalMemory::BashPattern("ls*".to_string())),
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id, test_model());
        for event in &all_events {
            apply_event_to_state(&mut replayed_state, event);
        }

        assert!(
            replayed_state.approved_bash_patterns.contains("ls*"),
            "Bash pattern should be approved after replay"
        );
    }

    #[test]
    fn replay_empty_events_is_noop() {
        let session_id = deterministic_session_id();
        let mut state = AppState::new(session_id, test_model());

        let initial_message_count = state.conversation.messages.len();
        let initial_event_seq = state.event_sequence;

        let _ = reduce(
            &mut state,
            Action::Hydrate {
                session_id,
                events: vec![],
                starting_sequence: 0,
            },
        );

        assert_eq!(
            state.conversation.messages.len(),
            initial_message_count,
            "No messages should be added"
        );
        assert_eq!(
            state.event_sequence, 0,
            "Event sequence should be set to starting_sequence"
        );
    }

    #[test]
    fn replay_incremental_events_works() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id, test_model());
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);
        let effects = reduce(
            &mut live_state,
            Action::UserInput {
                session_id,
                text: NonEmptyString::new("Hello").unwrap(),
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: test_model(),
            },
        );
        let first_batch = collect_events(&effects);
        all_events.extend(first_batch.clone());

        let mut partial_state = AppState::new(session_id, test_model());
        for event in &first_batch {
            apply_event_to_state(&mut partial_state, event);
        }

        let effects = reduce(
            &mut live_state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id: deterministic_message_id("msg_2"),
                content: vec![AssistantContent::Text {
                    text: "Hi!".to_string(),
                }],
                timestamp: 1001,
            },
        );
        let second_batch = collect_events(&effects);
        all_events.extend(second_batch.clone());

        for event in &second_batch {
            apply_event_to_state(&mut partial_state, event);
        }

        assert_eq!(
            live_state.conversation.messages.len(),
            partial_state.conversation.messages.len(),
            "Incremental replay should produce same state"
        );
    }
}
