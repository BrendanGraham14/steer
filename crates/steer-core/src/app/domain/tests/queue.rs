#[cfg(test)]
mod tests {
    use crate::app::conversation::{AssistantContent, UserContent};
    use crate::app::domain::action::Action;
    use crate::app::domain::effect::Effect;
    use crate::app::domain::event::SessionEvent;
    use crate::app::domain::reduce::{InvalidActionKind, ReduceError, reduce};
    use crate::app::domain::state::{AppState, OperationKind, OperationState, QueuedWorkItem};
    use crate::app::domain::types::{MessageId, OpId, SessionId};
    use crate::config::model::builtin;
    use std::collections::HashSet;

    fn active_state(session_id: SessionId, op_id: OpId) -> AppState {
        let mut state = AppState::new(session_id);
        state.current_operation = Some(OperationState {
            op_id,
            kind: OperationKind::AgentLoop,
            pending_tool_calls: HashSet::new(),
        });
        state
    }

    fn reduce_ok(state: &mut AppState, action: Action) -> Vec<Effect> {
        reduce(state, action).expect("reduce failed")
    }

    #[test]
    fn queues_user_input_when_busy() {
        let session_id = SessionId::new();
        let active_op = OpId::new();
        let mut state = active_state(session_id, active_op);

        let queued_op = OpId::new();
        let message_id = MessageId::from_string("queued_msg");
        let model = builtin::claude_sonnet_4_5();
        let effects = reduce_ok(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Hello".to_string(),
                }],
                op_id: queued_op,
                message_id: message_id.clone(),
                model,
                timestamp: 1,
            },
        );

        assert_eq!(state.queued_work.len(), 1);
        match state.queued_work.front() {
            Some(QueuedWorkItem::UserMessage(item)) => {
                let joined_text = item
                    .content
                    .iter()
                    .filter_map(|content| match content {
                        UserContent::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                assert_eq!(joined_text, "Hello");
                assert_eq!(item.op_id, queued_op);
                assert_eq!(item.message_id, message_id);
            }
            _ => panic!("Expected queued user message"),
        }

        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::EmitEvent {
                    event: SessionEvent::QueueUpdated { .. },
                    ..
                }
            )),
            "Expected QueueUpdated event"
        );
    }

    #[test]
    fn coalesces_user_messages_when_busy() {
        let session_id = SessionId::new();
        let active_op = OpId::new();
        let mut state = active_state(session_id, active_op);

        let model = builtin::claude_sonnet_4_5();

        let _ = reduce_ok(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "First".to_string(),
                }],
                op_id: OpId::new(),
                message_id: MessageId::from_string("m1"),
                model: model.clone(),
                timestamp: 1,
            },
        );

        let _ = reduce_ok(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Second".to_string(),
                }],
                op_id: OpId::new(),
                message_id: MessageId::from_string("m2"),
                model,
                timestamp: 2,
            },
        );

        assert_eq!(state.queued_work.len(), 1);
        match state.queued_work.front() {
            Some(QueuedWorkItem::UserMessage(item)) => {
                let joined_text = item
                    .content
                    .iter()
                    .filter_map(|content| match content {
                        UserContent::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                assert_eq!(joined_text, "First\n\nSecond");
            }
            _ => panic!("Expected queued user message"),
        }
    }

    #[test]
    fn queues_direct_bash_separately() {
        let session_id = SessionId::new();
        let active_op = OpId::new();
        let mut state = active_state(session_id, active_op);

        let _ = reduce_ok(
            &mut state,
            Action::DirectBashCommand {
                session_id,
                op_id: OpId::new(),
                message_id: MessageId::from_string("b1"),
                command: "ls".to_string(),
                timestamp: 1,
            },
        );

        let _ = reduce_ok(
            &mut state,
            Action::DirectBashCommand {
                session_id,
                op_id: OpId::new(),
                message_id: MessageId::from_string("b2"),
                command: "pwd".to_string(),
                timestamp: 2,
            },
        );

        assert_eq!(state.queued_work.len(), 2);
        match state.queued_work.front() {
            Some(QueuedWorkItem::DirectBash(item)) => {
                assert_eq!(item.command, "ls");
            }
            _ => panic!("Expected queued bash command"),
        }
    }

    #[test]
    fn dequeues_next_item_after_completion() {
        let session_id = SessionId::new();
        let active_op = OpId::new();
        let mut state = active_state(session_id, active_op);
        let model = builtin::claude_sonnet_4_5();
        state.operation_models.insert(active_op, model.clone());

        let queued_op = OpId::new();
        let queued_message_id = MessageId::from_string("queued_user");
        let _ = reduce_ok(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Queued".to_string(),
                }],
                op_id: queued_op,
                message_id: queued_message_id,
                model: model.clone(),
                timestamp: 10,
            },
        );

        let effects = reduce_ok(
            &mut state,
            Action::ModelResponseComplete {
                session_id,
                op_id: active_op,
                message_id: MessageId::from_string("assistant"),
                content: vec![AssistantContent::Text {
                    text: "done".to_string(),
                }],
                usage: None,
                context_window_tokens: None,
                configured_max_output_tokens: None,
                timestamp: 11,
            },
        );

        assert!(state.queued_work.is_empty());
        let current_op = state.current_operation.as_ref().expect("expected new op");
        assert_eq!(current_op.op_id, queued_op);

        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::EmitEvent {
                    event: SessionEvent::OperationStarted { op_id, .. },
                    ..
                } if *op_id == queued_op
            )),
            "Expected OperationStarted for queued item"
        );
    }

    #[test]
    fn rejects_edit_and_compaction_when_busy() {
        let session_id = SessionId::new();
        let active_op = OpId::new();
        let mut state = active_state(session_id, active_op);

        let edit_result = reduce(
            &mut state,
            Action::UserEditedMessage {
                session_id,
                message_id: MessageId::from_string("orig"),
                new_content: vec![UserContent::Text {
                    text: "edit".to_string(),
                }],
                op_id: OpId::new(),
                new_message_id: MessageId::from_string("new"),
                model: builtin::claude_sonnet_4_5(),
                timestamp: 1,
            },
        );
        assert!(
            matches!(
                edit_result,
                Err(ReduceError::InvalidAction {
                    kind: InvalidActionKind::OperationInFlight,
                    ..
                })
            ),
            "Expected edit to be rejected while busy"
        );

        let compact_result = reduce(
            &mut state,
            Action::RequestCompaction {
                session_id,
                op_id: OpId::new(),
                model: builtin::claude_sonnet_4_5(),
            },
        );
        assert!(
            matches!(
                compact_result,
                Err(ReduceError::InvalidAction {
                    kind: InvalidActionKind::OperationInFlight,
                    ..
                })
            ),
            "Expected compaction to be rejected while busy"
        );
    }

    #[test]
    fn cancel_pops_queued_item_without_starting_it() {
        let session_id = SessionId::new();
        let active_op = OpId::new();
        let mut state = active_state(session_id, active_op);
        let model = builtin::claude_sonnet_4_5();
        state.operation_models.insert(active_op, model.clone());

        let queued_op = OpId::new();
        let queued_message_id = MessageId::from_string("queued_msg");
        let _ = reduce_ok(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Queued".to_string(),
                }],
                op_id: queued_op,
                message_id: queued_message_id.clone(),
                model,
                timestamp: 1,
            },
        );

        let effects = reduce_ok(
            &mut state,
            Action::Cancel {
                session_id,
                op_id: None,
            },
        );

        assert!(state.current_operation.is_none());
        assert!(state.queued_work.is_empty());

        let cancelled = effects.iter().find_map(|effect| match effect {
            Effect::EmitEvent {
                event: SessionEvent::OperationCancelled { info, .. },
                ..
            } => Some(info),
            _ => None,
        });
        let info = cancelled.expect("Expected OperationCancelled event");
        let popped = info
            .popped_queued_item
            .as_ref()
            .expect("Expected popped queued item");
        assert_eq!(popped.content, "Queued");
        assert_eq!(popped.op_id, queued_op);
        assert_eq!(popped.message_id, queued_message_id);

        assert!(
            !effects.iter().any(|effect| matches!(
                effect,
                Effect::EmitEvent {
                    event: SessionEvent::OperationStarted { .. },
                    ..
                }
            )),
            "Queued item should not auto-start after cancellation"
        );
    }

    #[test]
    fn dequeue_removes_head_without_starting() {
        let session_id = SessionId::new();
        let active_op = OpId::new();
        let mut state = active_state(session_id, active_op);

        let model = builtin::claude_sonnet_4_5();
        let _ = reduce_ok(
            &mut state,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "Queued".to_string(),
                }],
                op_id: OpId::new(),
                message_id: MessageId::from_string("queued_msg"),
                model,
                timestamp: 1,
            },
        );

        let effects = reduce_ok(&mut state, Action::DequeueQueuedItem { session_id });

        assert!(state.queued_work.is_empty());
        assert!(state.current_operation.is_some());
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::EmitEvent {
                    event: SessionEvent::QueueUpdated { .. },
                    ..
                }
            )),
            "Expected QueueUpdated event"
        );
    }
}
