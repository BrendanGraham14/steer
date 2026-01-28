use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

use super::message::{AssistantContent, Message, MessageData, UserContent};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageGraph {
    pub messages: Vec<Message>,
    pub active_message_id: Option<String>,
}

impl Default for MessageGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageGraph {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            active_message_id: None,
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.active_message_id = Some(message.id().to_string());
        self.messages.push(message);
    }

    pub fn add_message_from_data(&mut self, message_data: MessageData) -> &Message {
        debug!(target: "message_graph::add_message", "Adding message: {:?}", message_data);
        self.messages.push(Message {
            data: message_data,
            id: Message::generate_id("", Message::current_timestamp()),
            timestamp: Message::current_timestamp(),
            parent_message_id: self.active_message_id.clone(),
        });
        self.active_message_id = Some(self.messages.last().unwrap().id().to_string());
        self.messages.last().unwrap()
    }

    pub fn clear(&mut self) {
        debug!(target:"message_graph::clear", "Clearing message graph");
        self.messages.clear();
        self.active_message_id = None;
    }

    pub fn find_tool_name_by_id(&self, tool_id: &str) -> Option<String> {
        for message in &self.messages {
            if let MessageData::Assistant { content, .. } = &message.data {
                for content_block in content {
                    if let AssistantContent::ToolCall { tool_call, .. } = content_block {
                        if tool_call.id == tool_id {
                            return Some(tool_call.name.clone());
                        }
                    }
                }
            }
        }
        None
    }

    pub fn edit_message(
        &mut self,
        message_id: &str,
        new_content: Vec<UserContent>,
    ) -> Option<String> {
        let message_to_edit = self.messages.iter().find(|m| m.id() == message_id)?;

        if !matches!(&message_to_edit.data, MessageData::User { .. }) {
            return None;
        }

        let parent_id = message_to_edit.parent_message_id().map(|s| s.to_string());

        let new_message_id = Message::generate_id("user", Message::current_timestamp());
        let edited_message = Message {
            data: MessageData::User {
                content: new_content,
            },
            timestamp: Message::current_timestamp(),
            id: new_message_id.clone(),
            parent_message_id: parent_id,
        };

        self.messages.push(edited_message);
        self.active_message_id = Some(new_message_id.clone());

        Some(new_message_id)
    }

    pub fn update_command_execution(
        &mut self,
        message_id: &str,
        command: String,
        stdout: String,
        stderr: String,
        exit_code: i32,
    ) -> Option<Message> {
        for message in &mut self.messages {
            if message.id() != message_id {
                continue;
            }

            if let MessageData::User { content } = &mut message.data {
                *content = vec![UserContent::CommandExecution {
                    command,
                    stdout,
                    stderr,
                    exit_code,
                }];
                return Some(message.clone());
            }

            return None;
        }

        None
    }

    pub fn replace_message(&mut self, updated: Message) -> bool {
        for message in &mut self.messages {
            if message.id() == updated.id() {
                *message = updated;
                return true;
            }
        }

        self.messages.push(updated);
        false
    }

    pub fn checkout(&mut self, message_id: &str) -> bool {
        if self.messages.iter().any(|m| m.id() == message_id) {
            self.active_message_id = Some(message_id.to_string());
            true
        } else {
            false
        }
    }

    pub fn get_active_thread(&self) -> Vec<&Message> {
        if self.messages.is_empty() {
            return Vec::new();
        }

        let head_id = if let Some(ref active_id) = self.active_message_id {
            active_id.as_str()
        } else {
            self.messages.last().map_or("", |m| m.id())
        };

        let mut current_msg = self.messages.iter().find(|m| m.id() == head_id);
        if current_msg.is_none() {
            current_msg = self.messages.last();
        }

        let mut result = Vec::new();
        let id_map: HashMap<&str, &Message> = self.messages.iter().map(|m| (m.id(), m)).collect();

        while let Some(msg) = current_msg {
            result.push(msg);

            current_msg = if let Some(parent_id) = msg.parent_message_id() {
                id_map.get(parent_id).copied()
            } else {
                None
            };
        }

        result.reverse();

        debug!(
            "Active thread: [{}]",
            result
                .iter()
                .map(|msg| msg.id())
                .collect::<Vec<_>>()
                .join(", ")
        );
        result
    }

    pub fn get_thread_messages(&self) -> Vec<&Message> {
        self.get_active_thread()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_user_message(id: &str, parent_id: Option<&str>, content: &str) -> Message {
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: content.to_string(),
                }],
            },
            timestamp: Message::current_timestamp(),
            id: id.to_string(),
            parent_message_id: parent_id.map(String::from),
        }
    }

    fn create_assistant_message(id: &str, parent_id: Option<&str>, content: &str) -> Message {
        Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: content.to_string(),
                }],
            },
            timestamp: Message::current_timestamp(),
            id: id.to_string(),
            parent_message_id: parent_id.map(String::from),
        }
    }

    #[test]
    fn test_editing_message_in_the_middle_of_conversation() {
        let mut graph = MessageGraph::new();

        let msg1 = create_user_message("msg1", None, "What is Rust?");
        graph.add_message(msg1.clone());

        let msg2 =
            create_assistant_message("msg2", Some("msg1"), "A systems programming language.");
        graph.add_message(msg2.clone());

        let msg3 = create_user_message("msg3", Some("msg2"), "Is it fast?");
        graph.add_message(msg3.clone());

        let msg4 = create_assistant_message("msg4", Some("msg3"), "Yes, it is very fast.");
        graph.add_message(msg4.clone());

        let edited_id = graph
            .edit_message(
                "msg1",
                vec![UserContent::Text {
                    text: "What is Golang?".to_string(),
                }],
            )
            .unwrap();

        let messages_after_edit = graph.get_thread_messages();
        let message_ids_after_edit: Vec<&str> =
            messages_after_edit.iter().map(|m| m.id()).collect();

        assert_eq!(
            message_ids_after_edit.len(),
            1,
            "Active thread should only show the edited message"
        );
        assert_eq!(message_ids_after_edit[0], edited_id.as_str());

        assert!(graph.messages.iter().any(|m| m.id() == "msg1"));
        assert!(graph.messages.iter().any(|m| m.id() == "msg2"));
        assert!(graph.messages.iter().any(|m| m.id() == "msg3"));
        assert!(graph.messages.iter().any(|m| m.id() == "msg4"));

        let msg5 = create_assistant_message(
            "msg5",
            Some(&edited_id),
            "A systems programming language from Google.",
        );
        graph.add_message(msg5.clone());

        let final_messages = graph.get_thread_messages();
        let final_message_ids: Vec<&str> = final_messages.iter().map(|m| m.id()).collect();

        assert_eq!(
            final_messages.len(),
            2,
            "Should have the edited message and the new response."
        );
        assert_eq!(final_message_ids[0], edited_id.as_str());
        assert_eq!(final_message_ids[1], "msg5");
    }

    #[test]
    fn test_get_thread_messages_after_edit() {
        let mut graph = MessageGraph::new();

        let msg1 = create_user_message("msg1", None, "hello");
        graph.add_message(msg1.clone());

        let msg2 = create_assistant_message("msg2", Some("msg1"), "world");
        graph.add_message(msg2.clone());

        let msg3_original = create_user_message("msg3_original", Some("msg2"), "thanks");
        graph.add_message(msg3_original.clone());

        let edited_id = graph
            .edit_message(
                "msg3_original",
                vec![UserContent::Text {
                    text: "how are you".to_string(),
                }],
            )
            .unwrap();

        let msg4 = create_assistant_message("msg4", Some(&edited_id), "I am fine");
        graph.add_message(msg4.clone());

        let thread_messages = graph.get_thread_messages();

        let thread_message_ids: Vec<&str> = thread_messages.iter().map(|m| m.id()).collect();

        assert_eq!(
            thread_message_ids.len(),
            4,
            "Should have 4 messages in the current thread"
        );
        assert!(thread_message_ids.contains(&"msg1"), "Should contain msg1");
        assert!(thread_message_ids.contains(&"msg2"), "Should contain msg2");
        assert!(
            thread_message_ids.contains(&edited_id.as_str()),
            "Should contain the edited message"
        );
        assert!(thread_message_ids.contains(&"msg4"), "Should contain msg4");

        assert!(
            graph.messages.iter().any(|m| m.id() == "msg3_original"),
            "Original message should still exist in message history"
        );
    }

    #[test]
    fn test_get_thread_messages_filters_other_branches() {
        let mut graph = MessageGraph::new();

        let msg1 = create_user_message("msg1", None, "hi");
        graph.add_message(msg1.clone());

        let msg2 = create_assistant_message("msg2", Some("msg1"), "Hello! How can I help?");
        graph.add_message(msg2.clone());

        let msg3_original = create_user_message("msg3_original", Some("msg2"), "thanks");
        graph.add_message(msg3_original.clone());

        let msg4_original =
            create_assistant_message("msg4_original", Some("msg3_original"), "You're welcome!");
        graph.add_message(msg4_original.clone());

        let edited_id = graph
            .edit_message(
                "msg3_original",
                vec![UserContent::Text {
                    text: "how are you".to_string(),
                }],
            )
            .unwrap();

        let msg4_new = create_assistant_message(
            "msg4_new",
            Some(&edited_id),
            "I'm doing well, thanks for asking! Ready to help with any software engineering tasks you have.",
        );
        graph.add_message(msg4_new.clone());

        let msg5 = create_user_message("msg5", Some("msg4_new"), "what messages have I sent you?");
        graph.add_message(msg5.clone());

        let thread_messages = graph.get_thread_messages();

        let user_messages: Vec<String> = thread_messages
            .iter()
            .filter(|m| matches!(m.data, MessageData::User { .. }))
            .map(|m| m.extract_text())
            .collect();

        println!("User messages seen: {user_messages:?}");

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

        assert!(
            !user_messages.contains(&"thanks".to_string()),
            "Should NOT contain 'thanks' from the non-active branch"
        );

        assert!(
            graph.messages.iter().any(|m| m.id() == "msg3_original"),
            "Original 'thanks' message should still exist in message history"
        );
    }

    #[test]
    fn test_checkout_branch() {
        let mut graph = MessageGraph::new();

        let msg1 = create_user_message("msg1", None, "hello");
        graph.add_message(msg1.clone());

        let msg2 = create_assistant_message("msg2", Some("msg1"), "hi there");
        graph.add_message(msg2.clone());

        let edited_id = graph
            .edit_message(
                "msg1",
                vec![UserContent::Text {
                    text: "goodbye".to_string(),
                }],
            )
            .unwrap();

        assert_eq!(graph.active_message_id, Some(edited_id.clone()));
        let thread = graph.get_active_thread();
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].id(), edited_id);

        assert!(graph.checkout("msg2"));
        assert_eq!(graph.active_message_id, Some("msg2".to_string()));

        let thread = graph.get_active_thread();
        assert_eq!(thread.len(), 2);
        assert_eq!(thread[0].id(), "msg1");
        assert_eq!(thread[1].id(), "msg2");

        assert!(!graph.checkout("non-existent"));
        assert_eq!(graph.active_message_id, Some("msg2".to_string()));
    }

    #[test]
    fn test_active_message_id_tracking() {
        let mut graph = MessageGraph::new();

        assert_eq!(graph.active_message_id, None);

        let msg1 = create_user_message("msg1", None, "hello");
        graph.add_message(msg1);
        assert_eq!(graph.active_message_id, Some("msg1".to_string()));

        let msg2 = create_assistant_message("msg2", Some("msg1"), "hi");
        graph.add_message(msg2);
        assert_eq!(graph.active_message_id, Some("msg2".to_string()));

        let msg3 = create_user_message("msg3", Some("msg1"), "different question");
        graph.add_message(msg3);
        assert_eq!(graph.active_message_id, Some("msg3".to_string()));

        let msg4 = create_user_message("msg4", Some("msg3"), "follow up");
        graph.add_message(msg4);
        assert_eq!(graph.active_message_id, Some("msg4".to_string()));
    }
}
