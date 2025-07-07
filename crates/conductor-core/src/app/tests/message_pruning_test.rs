use crate::app::conversation::{AssistantContent, Conversation, Message, UserContent};
use uuid::Uuid;

fn create_user_message(
    id: &str,
    parent_id: Option<&str>,
    thread_id: Uuid,
    content: &str,
) -> Message {
    Message::User {
        id: id.to_string(),
        parent_message_id: parent_id.map(String::from),
        thread_id,
        content: vec![UserContent::Text {
            text: content.to_string(),
        }],
        timestamp: Message::current_timestamp(),
    }
}

fn create_assistant_message(
    id: &str,
    parent_id: Option<&str>,
    thread_id: Uuid,
    content: &str,
) -> Message {
    Message::Assistant {
        id: id.to_string(),
        parent_message_id: parent_id.map(String::from),
        thread_id,
        content: vec![AssistantContent::Text {
            text: content.to_string(),
        }],
        timestamp: Message::current_timestamp(),
    }
}

#[test]
fn test_editing_message_in_the_middle_of_conversation() {
    let mut conversation = Conversation::new();
    let initial_thread_id = conversation.current_thread_id;

    // 1. Build an initial conversation
    let msg1 = create_user_message("msg1", None, initial_thread_id, "What is Rust?");
    conversation.add_message(msg1.clone());

    let msg2 = create_assistant_message(
        "msg2",
        Some("msg1"),
        initial_thread_id,
        "A systems programming language.",
    );
    conversation.add_message(msg2.clone());

    let msg3 = create_user_message("msg3", Some("msg2"), initial_thread_id, "Is it fast?");
    conversation.add_message(msg3.clone());

    let msg4 = create_assistant_message(
        "msg4",
        Some("msg3"),
        initial_thread_id,
        "Yes, it is very fast.",
    );
    conversation.add_message(msg4.clone());

    // 2. Edit the *first* user message
    let new_thread_id = conversation
        .edit_message(
            "msg1",
            vec![UserContent::Text {
                text: "What is Golang?".to_string(),
            }],
        )
        .unwrap();

    // 3. Check the state after editing
    let edited_msg_id = {
        let messages_after_edit = conversation.get_thread_messages();
        let message_ids_after_edit: Vec<&str> =
            messages_after_edit.iter().map(|m| m.id()).collect();

        assert_eq!(
            message_ids_after_edit.len(),
            1,
            "History should be pruned to the single edited message."
        );
        let id = message_ids_after_edit[0];
        assert_ne!(id, "msg1");
        assert!(!message_ids_after_edit.contains(&"msg1"));
        assert!(!message_ids_after_edit.contains(&"msg2"));
        assert!(!message_ids_after_edit.contains(&"msg3"));
        assert!(!message_ids_after_edit.contains(&"msg4"));
        id.to_string()
    };

    // 4. Add a new message to the new branch of conversation
    let msg5 = create_assistant_message(
        "msg5",
        Some(&edited_msg_id),
        new_thread_id,
        "A systems programming language from Google.",
    );
    conversation.add_message(msg5.clone());

    // 5. Check the final state of the conversation
    let final_messages = conversation.get_thread_messages();
    let final_message_ids: Vec<&str> = final_messages.iter().map(|m| m.id()).collect();

    assert_eq!(
        final_messages.len(),
        2,
        "Should have the edited message and the new response."
    );
    assert_eq!(final_message_ids[0], edited_msg_id);
    assert_eq!(final_message_ids[1], "msg5");
}
