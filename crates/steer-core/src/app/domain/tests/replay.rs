#[cfg(test)]
mod tests {
    use crate::api::provider::TokenUsage;
    use crate::app::conversation::{AssistantContent, UserContent};
    use crate::app::domain::action::{Action, ApprovalDecision, ApprovalMemory};
    use crate::app::domain::effect::Effect;
    use crate::app::domain::event::{ContextWindowUsage, SessionEvent};
    use crate::app::domain::reduce::{apply_event_to_state, reduce};
    use crate::app::domain::state::{AppState, OperationKind, OperationState};
    use crate::app::domain::types::{CompactionId, MessageId, OpId, RequestId, SessionId};
    use crate::config::model::builtin;
    use std::collections::HashSet;
    use steer_tools::ToolCall;

    fn test_model() -> crate::config::model::ModelId {
        builtin::claude_sonnet_4_5()
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

    fn reduce_ok(state: &mut AppState, action: Action) -> Vec<Effect> {
        reduce(state, action).expect("reduce failed")
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

        let mut live_state = AppState::new(session_id);
        let effects = reduce_ok(
            &mut live_state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Hello world".to_string(),
                }],
                op_id,
                message_id,
                timestamp: 1000,
                model: test_model(),
            },
        );

        let events = collect_events(&effects);

        let mut replayed_state = AppState::new(session_id);
        for event in &events {
            apply_event_to_state(&mut replayed_state, event);
        }

        assert_eq!(
            live_state.message_graph.messages.len(),
            replayed_state.message_graph.messages.len(),
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
        let mut live_state = AppState::new(session_id);
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);
        let effects = reduce_ok(
            &mut live_state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "List files".to_string(),
                }],
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
        let effects = reduce_ok(
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
                        thought_signature: None,
                    },
                ],
                usage: None,
                context_window_tokens: None,
                configured_max_output_tokens: None,
                timestamp: 1001,
            },
        );
        all_events.extend(collect_events(&effects));

        let request_id = live_state.pending_approval.as_ref().unwrap().request_id;
        let effects = reduce_ok(
            &mut live_state,
            Action::ToolApprovalDecided {
                session_id,
                request_id,
                decision: ApprovalDecision::Approved,
                remember: Some(ApprovalMemory::Tool("bash".to_string())),
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id);
        for event in &all_events {
            apply_event_to_state(&mut replayed_state, event);
        }

        assert_eq!(
            live_state.message_graph.messages.len(),
            replayed_state.message_graph.messages.len(),
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
        let mut live_state = AppState::new(session_id);
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);
        let effects = reduce_ok(
            &mut live_state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Hello".to_string(),
                }],
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: test_model(),
            },
        );
        all_events.extend(collect_events(&effects));

        let effects = reduce_ok(
            &mut live_state,
            Action::Cancel {
                session_id,
                op_id: None,
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id);
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
    fn replay_multiple_approvals_produces_same_state() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id);
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
                name: (*tool_name).to_string(),
                parameters: serde_json::json!({}),
            };

            let effects = reduce_ok(
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
        let effects = reduce_ok(
            &mut live_state,
            Action::ToolApprovalDecided {
                session_id,
                request_id: pending_request_id,
                decision: ApprovalDecision::Approved,
                remember: Some(ApprovalMemory::Tool("bash".to_string())),
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id);
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
        let mut live_state = AppState::new(session_id);
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);
        let effects = reduce_ok(
            &mut live_state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Hello".to_string(),
                }],
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: test_model(),
            },
        );
        all_events.extend(collect_events(&effects));

        let effects = reduce_ok(
            &mut live_state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id: deterministic_message_id("msg_2"),
                content: vec![AssistantContent::Text {
                    text: "Hello!".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                configured_max_output_tokens: None,
                timestamp: 1001,
            },
        );
        all_events.extend(collect_events(&effects));

        let mut hydrated_state = AppState::new(session_id);
        let _ = reduce_ok(
            &mut hydrated_state,
            Action::Hydrate {
                session_id,
                events: all_events.clone(),
                starting_sequence: all_events.len() as u64,
            },
        );

        let mut manually_replayed = AppState::new(session_id);
        for event in &all_events {
            apply_event_to_state(&mut manually_replayed, event);
        }

        assert_eq!(
            hydrated_state.message_graph.messages.len(),
            manually_replayed.message_graph.messages.len(),
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
        let mut live_state = AppState::new(session_id);
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);

        let effects = reduce_ok(
            &mut live_state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "First message".to_string(),
                }],
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: test_model(),
            },
        );
        all_events.extend(collect_events(&effects));

        let effects = reduce_ok(
            &mut live_state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id: deterministic_message_id("msg_2"),
                content: vec![AssistantContent::Text {
                    text: "First response".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                configured_max_output_tokens: None,
                timestamp: 1001,
            },
        );
        all_events.extend(collect_events(&effects));

        let op_id_2 = deterministic_op_id(2);
        let effects = reduce_ok(
            &mut live_state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Second message".to_string(),
                }],
                op_id: op_id_2,
                message_id: deterministic_message_id("msg_3"),
                timestamp: 2000,
                model: test_model(),
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id);
        for event in &all_events {
            apply_event_to_state(&mut replayed_state, event);
        }

        assert_eq!(
            live_state.message_graph.messages.len(),
            replayed_state.message_graph.messages.len(),
            "Should have same number of messages"
        );

        for (i, (live_msg, replayed_msg)) in live_state
            .message_graph
            .messages
            .iter()
            .zip(replayed_state.message_graph.messages.iter())
            .enumerate()
        {
            assert_eq!(
                live_msg.id(),
                replayed_msg.id(),
                "Message {i} ID should match"
            );
        }
    }

    #[test]
    fn replay_bash_pattern_approval() {
        let session_id = deterministic_session_id();
        let mut live_state = AppState::new(session_id);
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

        let effects = reduce_ok(
            &mut live_state,
            Action::ToolApprovalRequested {
                session_id,
                request_id,
                tool_call,
            },
        );
        let mut all_events = collect_events(&effects);

        let effects = reduce_ok(
            &mut live_state,
            Action::ToolApprovalDecided {
                session_id,
                request_id,
                decision: ApprovalDecision::Approved,
                remember: Some(ApprovalMemory::BashPattern("ls*".to_string())),
            },
        );
        all_events.extend(collect_events(&effects));

        let mut replayed_state = AppState::new(session_id);
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
        let mut state = AppState::new(session_id);

        let initial_message_count = state.message_graph.messages.len();

        let _ = reduce_ok(
            &mut state,
            Action::Hydrate {
                session_id,
                events: vec![],
                starting_sequence: 0,
            },
        );

        assert_eq!(
            state.message_graph.messages.len(),
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
        let mut live_state = AppState::new(session_id);
        let mut all_events = Vec::new();

        let op_id = deterministic_op_id(1);
        let effects = reduce_ok(
            &mut live_state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Hello".to_string(),
                }],
                op_id,
                message_id: deterministic_message_id("msg_1"),
                timestamp: 1000,
                model: test_model(),
            },
        );
        let first_batch = collect_events(&effects);
        all_events.extend(first_batch.clone());

        let mut partial_state = AppState::new(session_id);
        for event in &first_batch {
            apply_event_to_state(&mut partial_state, event);
        }

        let effects = reduce_ok(
            &mut live_state,
            Action::ModelResponseComplete {
                session_id,
                op_id,
                message_id: deterministic_message_id("msg_2"),
                content: vec![AssistantContent::Text {
                    text: "Hi!".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                configured_max_output_tokens: None,
                timestamp: 1001,
            },
        );
        let second_batch = collect_events(&effects);
        all_events.extend(second_batch.clone());

        for event in &second_batch {
            apply_event_to_state(&mut partial_state, event);
        }

        assert_eq!(
            live_state.message_graph.messages.len(),
            partial_state.message_graph.messages.len(),
            "Incremental replay should produce same state"
        );
    }

    #[test]
    fn hydrate_action_applies_llm_usage_update() {
        let session_id = deterministic_session_id();
        let op_id = deterministic_op_id(42);
        let model = test_model();
        let usage = TokenUsage::new(9, 13, 22);
        let context_window = Some(ContextWindowUsage {
            max_context_tokens: Some(200_000),
            remaining_tokens: Some(199_978),
            utilization_ratio: Some(0.00011),
            estimated: false,
        });

        let events = vec![SessionEvent::LlmUsageUpdated {
            op_id,
            model: model.clone(),
            usage,
            context_window: context_window.clone(),
        }];

        let mut hydrated_state = AppState::new(session_id);
        let _ = reduce_ok(
            &mut hydrated_state,
            Action::Hydrate {
                session_id,
                events,
                starting_sequence: 1,
            },
        );

        let snapshot = hydrated_state
            .llm_usage_by_op
            .get(&op_id)
            .expect("expected usage snapshot for hydrated op");
        assert_eq!(snapshot.model, model);
        assert_eq!(snapshot.usage, usage);
        assert_eq!(snapshot.context_window, context_window);
        assert_eq!(hydrated_state.llm_usage_totals, usage);
        assert_eq!(hydrated_state.event_sequence, 1);
    }

    #[test]
    fn test_compaction_replay_marks_boundary_in_state() {
        use crate::app::conversation::{Message, MessageData, UserContent};
        use crate::app::domain::action::Action;
        use crate::app::domain::event::CompactTrigger;
        use crate::app::domain::state::OperationKind;

        let session_id = deterministic_session_id();
        let op_id = deterministic_op_id(1);
        let mut live_state = AppState::new(session_id);

        // Add pre-compaction messages.
        let msg1 = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "hello".to_string(),
                }],
            },
            id: "msg1".to_string(),
            parent_message_id: None,
            timestamp: 1,
        };
        live_state.message_graph.add_message(msg1);
        let msg2 = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "hi there".to_string(),
                }],
            },
            id: "msg2".to_string(),
            parent_message_id: Some("msg1".to_string()),
            timestamp: 2,
        };
        live_state.message_graph.add_message(msg2);

        // Start Compact operation.
        live_state.start_operation(
            op_id,
            OperationKind::Compact {
                trigger: CompactTrigger::Manual,
            },
        );
        live_state
            .operation_models
            .insert(op_id, builtin::claude_sonnet_4_5());

        let summary_message_id = deterministic_message_id("summary");
        let compaction_id = CompactionId::new();

        let effects = reduce_ok(
            &mut live_state,
            Action::CompactionComplete {
                session_id,
                op_id,
                compaction_id,
                summary_message_id: summary_message_id.clone(),
                summary: "Summary of conversation.".to_string(),
                compacted_head_message_id: deterministic_message_id("msg2"),
                previous_active_message_id: Some(deterministic_message_id("msg2")),
                model: "claude-sonnet-4-5-20250929".to_string(),
                timestamp: 3,
            },
        );

        // Replay events onto a fresh state.
        let events = collect_events(&effects);
        let mut replayed_state = AppState::new(session_id);
        for event in &events {
            apply_event_to_state(&mut replayed_state, event);
        }

        // compaction_summary_ids should match.
        assert_eq!(
            live_state.compaction_summary_ids, replayed_state.compaction_summary_ids,
            "AppState.compaction_summary_ids should match after replay"
        );

        // MessageGraph compaction_summary_ids should match.
        assert_eq!(
            live_state.message_graph.compaction_summary_ids,
            replayed_state.message_graph.compaction_summary_ids,
            "MessageGraph.compaction_summary_ids should match after replay"
        );

        // get_thread_messages should return the same messages.
        let live_thread_ids: Vec<&str> = live_state
            .message_graph
            .get_thread_messages()
            .iter()
            .map(|m| m.id())
            .collect();
        let replayed_thread_ids: Vec<&str> = replayed_state
            .message_graph
            .get_thread_messages()
            .iter()
            .map(|m| m.id())
            .collect();
        assert_eq!(
            live_thread_ids, replayed_thread_ids,
            "get_thread_messages should return the same filtered list after replay"
        );

        // Enhancement: dispatch UserInput on replayed state and verify CallModel
        // messages are filtered (no pre-compaction messages).
        let post_op_id = deterministic_op_id(100);
        let post_effects = reduce_ok(
            &mut replayed_state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "post-replay question".to_string(),
                }],
                op_id: post_op_id,
                message_id: deterministic_message_id("post_replay_msg"),
                model: builtin::claude_sonnet_4_5(),
                timestamp: 10,
            },
        );

        let callmodel_messages = post_effects.iter().find_map(|e| match e {
            Effect::CallModel { messages, .. } => Some(messages),
            _ => None,
        });
        let messages =
            callmodel_messages.expect("expected CallModel effect from UserInput on replayed state");

        assert_eq!(
            messages.len(),
            2,
            "CallModel should contain exactly 2 messages (summary + new user msg), got: {:?}",
            messages.iter().map(|m| m.id()).collect::<Vec<_>>()
        );

        // First message should be the compaction summary.
        assert!(
            matches!(
                &messages[0].data,
                MessageData::Assistant { content }
                    if content.iter().any(|c| matches!(
                        c,
                        AssistantContent::Text { text } if text == "Summary of conversation."
                    ))
            ),
            "messages[0] should be the compaction summary"
        );

        // Second message should be the new user message.
        assert!(
            matches!(
                &messages[1].data,
                MessageData::User { content }
                    if content.iter().any(|c| matches!(
                        c,
                        UserContent::Text { text } if text == "post-replay question"
                    ))
            ),
            "messages[1] should be the new user message"
        );

        // Pre-compaction messages must NOT appear.
        let has_pre_compaction = messages.iter().any(|m| match &m.data {
            MessageData::User { content } => content.iter().any(|c| {
                matches!(
                    c,
                    UserContent::Text { text } if text == "hello"
                )
            }),
            MessageData::Assistant { content } => content.iter().any(|c| {
                matches!(
                    c,
                    AssistantContent::Text { text } if text == "hi there"
                )
            }),
            _ => false,
        });
        assert!(
            !has_pre_compaction,
            "CallModel messages must not contain pre-compaction messages after replay"
        );
    }
}
