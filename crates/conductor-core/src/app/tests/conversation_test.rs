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
fn test_get_thread_messages_after_edit() {
    let mut conversation = Conversation::new();
    let initial_thread_id = conversation.current_thread_id;

    // 1. Initial conversation
    let msg1 = create_user_message("msg1", None, initial_thread_id, "hello");
    conversation.add_message(msg1.clone());

    let msg2 = create_assistant_message("msg2", Some("msg1"), initial_thread_id, "world");
    conversation.add_message(msg2.clone());

    // This is the message that will be "edited out"
    let msg3_original =
        create_user_message("msg3_original", Some("msg2"), initial_thread_id, "thanks");
    conversation.add_message(msg3_original.clone());

    // 2. Edit the last user message ("thanks")
    let new_thread_id = conversation
        .edit_message(
            "msg3_original",
            vec![UserContent::Text {
                text: "how are you".to_string(),
            }],
        )
        .unwrap();

    let edited_msg_id = conversation.messages.last().unwrap().id().to_string();

    // 3. Add a new assistant message to the new branch
    let msg4 = create_assistant_message("msg4", Some(&edited_msg_id), new_thread_id, "I am fine");
    conversation.add_message(msg4.clone());

    // 4. Get messages for the current thread
    let thread_messages = conversation.get_thread_messages();

    // 5. Assertions
    let thread_message_ids: Vec<&str> = thread_messages.iter().map(|m| m.id()).collect();

    // Should contain the root, the first assistant message, the *new* user message, and the final assistant message
    assert_eq!(
        thread_message_ids.len(),
        4,
        "Should have 4 messages in the current thread"
    );
    assert!(thread_message_ids.contains(&"msg1"), "Should contain msg1");
    assert!(thread_message_ids.contains(&"msg2"), "Should contain msg2");
    assert!(
        thread_message_ids.contains(&edited_msg_id.as_str()),
        "Should contain the edited message"
    );
    assert!(thread_message_ids.contains(&"msg4"), "Should contain msg4");

    // CRITICAL: Should NOT contain the original, edited-out message
    assert!(
        !thread_message_ids.contains(&"msg3_original"),
        "Should NOT contain the original message that was edited"
    );
}

#[test]
fn test_get_thread_messages_filters_other_branches() {
    let mut conversation = Conversation::new();
    let initial_thread_id = conversation.current_thread_id;

    // 1. Initial conversation: "hi"
    let msg1 = create_user_message("msg1", None, initial_thread_id, "hi");
    conversation.add_message(msg1.clone());

    let msg2 = create_assistant_message(
        "msg2",
        Some("msg1"),
        initial_thread_id,
        "Hello! How can I help?",
    );
    conversation.add_message(msg2.clone());

    // 2. User says "thanks" (this will be edited out)
    let msg3_original =
        create_user_message("msg3_original", Some("msg2"), initial_thread_id, "thanks");
    conversation.add_message(msg3_original.clone());

    let msg4_original = create_assistant_message(
        "msg4_original",
        Some("msg3_original"),
        initial_thread_id,
        "You're welcome!",
    );
    conversation.add_message(msg4_original.clone());

    // 3. Edit the "thanks" message to "how are you"
    let new_thread_id = conversation
        .edit_message(
            "msg3_original",
            vec![UserContent::Text {
                text: "how are you".to_string(),
            }],
        )
        .unwrap();

    let edited_msg_id = conversation.messages.last().unwrap().id().to_string();

    // 4. Add assistant response in the new thread
    let msg4_new = create_assistant_message(
        "msg4_new",
        Some(&edited_msg_id),
        new_thread_id,
        "I'm doing well, thanks for asking! Ready to help with any software engineering tasks you have.",
    );
    conversation.add_message(msg4_new.clone());

    // 5. User asks "what messages have I sent you?"
    let msg5 = create_user_message(
        "msg5",
        Some("msg4_new"),
        new_thread_id,
        "what messages have I sent you?",
    );
    conversation.add_message(msg5.clone());

    // 6. Get messages for the current thread - this should NOT include "thanks"
    let thread_messages = conversation.get_thread_messages();

    // Extract the user messages
    let user_messages: Vec<String> = thread_messages
        .iter()
        .filter(|m| matches!(m, Message::User { .. }))
        .map(|m| m.extract_text())
        .collect();

    println!("User messages seen: {user_messages:?}");

    // Assertions
    assert_eq!(
        user_messages.len(),
        3,
        "Should have exactly 3 user messages"
    );
    assert_eq!(user_messages[0], "hi", "First message should be 'hi'");
    assert_eq!(
        user_messages[1], "how are you",
        "Second message should be 'how are you' (edited)"
    );
    assert_eq!(
        user_messages[2], "what messages have I sent you?",
        "Third message should be the question"
    );

    // CRITICAL: Should NOT contain "thanks" from the edited-out branch
    assert!(
        !user_messages.contains(&"thanks".to_string()),
        "Should NOT contain 'thanks' from the edited-out branch"
    );
}
