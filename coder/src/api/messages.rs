use serde::{Deserialize, Serialize};
use strum_macros::Display;
use tracing::{debug, error, warn};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display)]
pub enum MessageRole {
    #[serde(rename = "user")]
    #[strum(serialize = "user")]
    User,
    #[serde(rename = "assistant")]
    #[strum(serialize = "assistant")]
    Assistant,
    #[serde(rename = "tool")]
    #[strum(serialize = "tool")]
    Tool,
}

/// Represents a message in a conversation, adaptable to different provider APIs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: MessageRole,
    #[serde(flatten)]
    pub content: MessageContent,
    #[serde(skip_serializing)]
    pub id: Option<String>,
}

/// Content types for Claude API messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content
    Text { content: String },
    /// Structured content for tool results or other special content
    StructuredContent { content: StructuredContent },
}

impl MessageContent {
    pub fn extract_text(&self) -> String {
        match self {
            MessageContent::Text { content } => content.clone(),
            MessageContent::StructuredContent { content } => content
                .0
                .iter()
                .filter_map(|block| {
                    if let ContentBlock::Text { text } = block {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// Represents structured content blocks for messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(transparent)]
pub struct StructuredContent(pub Vec<ContentBlock>);

/// Different types of content blocks used in structured messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// A tool result block from executing a tool
    #[serde(rename = "tool_result")]
    ToolResult {
        /// ID of the tool use this result is for
        tool_use_id: String,
        /// Result content from the tool execution (must be an array of content blocks)
        content: Vec<ContentBlock>,
        /// Optional field indicating if the tool failed
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },

    /// A tool call from the assistant
    #[serde(rename = "tool_use")]
    ToolUse {
        /// Unique ID for this tool use
        id: String,
        /// Name of the tool being called
        name: String,
        /// Input parameters for the tool
        input: serde_json::Value,
    },

    /// Generic text content
    #[serde(rename = "text")]
    Text {
        /// Text content in this block
        text: String,
    },
}

pub fn convert_conversation(conversation: &crate::app::Conversation) -> Vec<Message> {
    let mut api_messages = Vec::new();
    // Use iter().peekable() to group consecutive tool messages
    let mut app_messages_iter = conversation.messages.iter().peekable();

    while let Some(app_msg) = app_messages_iter.next() {
        match app_msg.role {
            crate::app::conversation::Role::User => {
                // Handle User messages (should primarily be text blocks)
                // Combine consecutive text blocks into one string
                let combined_text = app_msg
                    .content_blocks
                    .iter()
                    .filter_map(|block| {
                        if let crate::app::conversation::MessageContentBlock::Text(content) = block
                        {
                            Some(content.as_str())
                        } else {
                            warn!(target: "messages.convert_conversation", "User message ID {} contained non-text block: {:?}", app_msg.id, block);
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                if !combined_text.trim().is_empty() {
                    api_messages.push(Message {
                        role: MessageRole::User,
                        content: MessageContent::Text {
                            content: combined_text,
                        },
                        id: Some(app_msg.id.clone()),
                    });
                } else {
                    debug!(target: "messages.convert_conversation", "Skipping empty user message with ID: {}", app_msg.id);
                }
            }
            crate::app::conversation::Role::Assistant => {
                // Assistant messages can contain multiple content blocks (text, tool_use)
                let api_blocks: Vec<ContentBlock> = app_msg.content_blocks.iter().filter_map(|block| {
                    match block {
                        crate::app::conversation::MessageContentBlock::Text(text) => {
                            // Include text block only if it's not empty
                            if text.trim().is_empty() {
                                None
                            } else {
                                Some(ContentBlock::Text { text: text.clone() })
                            }
                        },
                        crate::app::conversation::MessageContentBlock::ToolCall(tc) => {
                            Some(ContentBlock::ToolUse {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                input: tc.parameters.clone(),
                            })
                        },
                        crate::app::conversation::MessageContentBlock::ToolResult { .. } => {
                            // Assistant should not have ToolResult blocks
                            error!(target: "messages.convert_conversation", "Unexpected ToolResult block found in Assistant message ID: {}", app_msg.id);
                            None // Skip this invalid block
                        }
                    }
                }).collect();

                // Only add the message if there are valid content blocks
                if !api_blocks.is_empty() {
                    // Determine content structure: single text block or structured content
                    let api_content = if api_blocks.len() == 1 {
                        if let Some(ContentBlock::Text { text }) = api_blocks.first() {
                            MessageContent::Text {
                                content: text.clone(),
                            }
                        } else {
                            // Single non-text block (must be ToolUse)
                            MessageContent::StructuredContent {
                                content: StructuredContent(api_blocks),
                            }
                        }
                    } else {
                        // Multiple blocks (text + tool_use, or multiple tool_use)
                        MessageContent::StructuredContent {
                            content: StructuredContent(api_blocks),
                        }
                    };

                    api_messages.push(Message {
                        role: MessageRole::Assistant,
                        content: api_content,
                        id: Some(app_msg.id.clone()),
                    });
                } else {
                    warn!(target: "messages.convert_conversation", "Assistant message ID {} resulted in no valid content blocks, skipping.", app_msg.id);
                }
            }
            crate::app::conversation::Role::Tool => {
                // Group consecutive Tool messages into a single API user message
                let mut tool_results = Vec::new();
                // Use the ID of the first tool message in the sequence for the API message ID
                let first_tool_msg_id = app_msg.id.clone();

                // Process the first tool message's blocks
                for block in &app_msg.content_blocks {
                    if let crate::app::conversation::MessageContentBlock::ToolResult {
                        tool_use_id,
                        result,
                    } = block
                    {
                        let is_error = result.starts_with("Error:");
                        let result_content = if result.trim().is_empty() {
                            "(No output)".to_string()
                        } else {
                            result.clone()
                        };

                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: vec![ContentBlock::Text {
                                text: result_content,
                            }],
                            is_error: if is_error { Some(true) } else { None },
                        });
                    } else {
                        error!(target: "messages.convert_conversation", "Message ID {} (part of Tool group) has unexpected content block: {:?}", app_msg.id, block);
                    }
                }

                // Peek ahead and consume subsequent Tool messages
                while let Some(next_msg) = app_messages_iter.peek() {
                    if next_msg.role == crate::app::conversation::Role::Tool {
                        // Consume the message from the iterator
                        let consumed_msg = app_messages_iter.next().unwrap(); // Safe due to peek
                        // Process all blocks within the consumed tool message
                        for block in &consumed_msg.content_blocks {
                            if let crate::app::conversation::MessageContentBlock::ToolResult {
                                tool_use_id,
                                result,
                            } = block
                            {
                                let is_error = result.starts_with("Error:");
                                let result_content = if result.trim().is_empty() {
                                    "(No output)".to_string()
                                } else {
                                    result.clone()
                                };
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: tool_use_id.clone(),
                                    content: vec![ContentBlock::Text {
                                        text: result_content,
                                    }],
                                    is_error: if is_error { Some(true) } else { None },
                                });
                            } else {
                                error!(target: "messages.convert_conversation", "Message ID {} (part of Tool group) has unexpected content block: {:?}", consumed_msg.id, block);
                            }
                        }
                    } else {
                        // Next message is not Role::Tool, stop grouping
                        break;
                    }
                }

                // Add the grouped tool results as a single API user message
                if !tool_results.is_empty() {
                    api_messages.push(Message {
                        role: MessageRole::User,
                        content: MessageContent::StructuredContent {
                            content: StructuredContent(tool_results),
                        },
                        id: Some(first_tool_msg_id), // Use ID of the first tool message
                    });
                }
            }
        }
    }

    // Ensure the last message isn't an empty non-assistant *text* message.
    // API allows empty assistant messages and expects tool results.
    if let Some(last) = api_messages.last() {
        let mut remove_last = false;
        if last.role == MessageRole::User {
            if let MessageContent::Text { content } = &last.content {
                if content.trim().is_empty() {
                    remove_last = true;
                }
            }
        }
        // Optional: uncomment to also remove empty assistant text messages if they are last
        /* else if last.role == "assistant" {
            if let MessageContent::Text { content } = &last.content_type {
               if content.trim().is_empty() {
                   remove_last = true;
               }
           }
        } */

        if remove_last {
            warn!(target: "messages.convert_conversation", "Last message was an empty non-assistant text message, removing it.");
            api_messages.pop();
        }
    }

    api_messages
}

/// Converts content blocks from an API response (api::ContentBlock)
/// to content blocks suitable for constructing a new API message (messages::ContentBlock).
pub fn convert_api_content_to_message_content(
    api_content: Vec<crate::api::ContentBlock>,
) -> Vec<ContentBlock> {
    api_content
        .into_iter()
        .filter_map(|api_block| match api_block {
            crate::api::ContentBlock::Text { text, .. } => {
                // Only include non-empty text blocks
                if text.trim().is_empty() {
                    None
                } else {
                    Some(ContentBlock::Text { text })
                }
            }
            crate::api::ContentBlock::ToolUse {
                id, name, input, ..
            } => Some(ContentBlock::ToolUse { id, name, input }),
            crate::api::ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => Some(ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        Conversation, Message as AppMessage, MessageContentBlock as AppMessageContentBlock,
        ToolCall as AppToolCall, conversation,
    };

    #[test]
    fn test_convert_conversation_basic() {
        let mut conv = Conversation::new();
        conv.add_message(AppMessage::new_text(
            conversation::Role::User,
            "Hello".to_string(),
        ));
        conv.add_message(AppMessage::new_text(
            conversation::Role::Assistant,
            "Hi there!".to_string(),
        ));

        let messages = convert_conversation(&conv);

        assert_eq!(messages.len(), 2);

        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(
            messages[0].content,
            MessageContent::Text {
                content: "Hello".to_string()
            }
        );

        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(
            messages[1].content,
            MessageContent::Text {
                content: "Hi there!".to_string()
            }
        );
    }

    #[test]
    fn test_convert_conversation_with_system() {
        let mut conv = Conversation::new();
        conv.add_message(AppMessage::new_text(
            conversation::Role::User,
            "Hello".to_string(),
        ));
        conv.add_message(AppMessage::new_text(
            conversation::Role::Assistant,
            "Hi there!".to_string(),
        ));

        let messages = convert_conversation(&conv);

        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_convert_conversation_with_tool_results() {
        let mut conv = Conversation::new();
        conv.add_message(AppMessage::new_text(
            conversation::Role::User,
            "Hello".to_string(),
        ));
        conv.add_message(AppMessage::new_text(
            conversation::Role::Assistant,
            "Let me check something".to_string(),
        ));

        // Add tool result using typed enum
        conv.add_message(AppMessage::new_with_blocks(
            conversation::Role::Tool,
            vec![AppMessageContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                result: "Result 1".to_string(),
            }],
        ));

        let messages = convert_conversation(&conv);
        println!("Test messages: {:?}", messages);

        assert_eq!(messages.len(), 3); // Now we expect all messages to be preserved

        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(
            messages[0].content,
            MessageContent::Text {
                content: "Hello".to_string()
            }
        );

        // The assistant message is converted to a user message with structured content
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(
            messages[1].content,
            MessageContent::Text {
                content: "Let me check something".to_string()
            }
        );

        // The tool message is converted to a user message with text content
        assert_eq!(messages[2].role, MessageRole::User);
        match &messages[2].content {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 1); // Expect one ToolResult block
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content: result_blocks, // Updated field name
                    is_error,
                } = &array[0]
                {
                    assert_eq!(tool_use_id, "tool_1");
                    assert!(is_error.is_none() || !is_error.unwrap()); // Check it's not an error
                    assert_eq!(result_blocks.len(), 1);
                    if let ContentBlock::Text { text } = &result_blocks[0] {
                        assert_eq!(text, "Result 1");
                    } else {
                        panic!("Expected inner Text block");
                    }
                } else {
                    panic!("Expected ToolResult block inside StructuredContent");
                }
            }
            _ => panic!("Expected StructuredContent for tool result"),
        }
    }

    #[test]
    fn test_convert_conversation_with_multiple_tool_results() {
        let mut conv = Conversation::new();
        conv.add_message(AppMessage::new_text(
            conversation::Role::User,
            "Hello".to_string(),
        ));
        // Use new_text constructor
        conv.add_message(AppMessage::new_text(
            conversation::Role::Assistant,
            "Let me check something".to_string(),
        ));

        // Add two tool results using new constructor
        conv.add_message(AppMessage::new_with_blocks(
            conversation::Role::Tool,
            vec![AppMessageContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                result: "Result 1".to_string(),
            }],
        ));
        conv.add_message(AppMessage::new_with_blocks(
            conversation::Role::Tool,
            vec![AppMessageContentBlock::ToolResult {
                tool_use_id: "tool_2".to_string(),
                result: "Result 2".to_string(),
            }],
        ));

        let messages = convert_conversation(&conv);
        println!("Multiple tool messages: {:?}", messages);

        assert_eq!(messages.len(), 3); // Now expecting 3 messages (user, assistant, and 1 tool result)

        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(
            messages[0].content,
            MessageContent::Text {
                content: "Hello".to_string()
            }
        );

        // The assistant message is converted to a user message with structured content for the first tool
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(
            messages[1].content,
            MessageContent::Text {
                content: "Let me check something".to_string()
            }
        );

        // The tool result is grouped into a single user message with StructuredContent
        assert_eq!(messages[2].role, MessageRole::User);
        match &messages[2].content {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 2);

                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content: result_blocks, // Updated field name
                    is_error: _,            // Ignore is_error
                } = &array[0]
                {
                    assert_eq!(tool_use_id, "tool_1");
                    assert_eq!(result_blocks.len(), 1);
                    if let ContentBlock::Text { text } = &result_blocks[0] {
                        assert_eq!(text, "Result 1");
                    } else {
                        panic!("Expected inner Text block at index 0");
                    }
                } else {
                    panic!("Expected ToolResult at index 0");
                }

                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content: result_blocks, // Updated field name
                    is_error: _,            // Ignore is_error
                } = &array[1]
                {
                    assert_eq!(tool_use_id, "tool_2");
                    assert_eq!(result_blocks.len(), 1);
                    if let ContentBlock::Text { text } = &result_blocks[0] {
                        assert_eq!(text, "Result 2");
                    } else {
                        panic!("Expected inner Text block at index 1");
                    }
                } else {
                    panic!("Expected ToolResult at index 1");
                }
            }
            _ => panic!("Expected StructuredContent for grouped tool results"),
        }
    }

    #[test]
    fn test_convert_conversation_with_empty_tool_results() {
        let mut conv = Conversation::new();
        conv.add_message(AppMessage::new_text(
            conversation::Role::User,
            "Hello".to_string(),
        ));
        // Use new_text constructor
        conv.add_message(AppMessage::new_text(
            conversation::Role::Assistant,
            "Let me check something".to_string(),
        ));
        // Add empty tool result
        conv.add_message(AppMessage::new_with_blocks(
            conversation::Role::Tool,
            vec![AppMessageContentBlock::ToolResult {
                tool_use_id: "empty_tool".to_string(),
                result: "".to_string(),
            }],
        ));

        let messages = convert_conversation(&conv);
        println!("Empty tool messages: {:?}", messages);

        assert_eq!(messages.len(), 3);

        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(
            messages[0].content,
            MessageContent::Text {
                content: "Hello".to_string()
            }
        );

        // Second message is a structured user message with tool result
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(
            messages[1].content,
            MessageContent::Text {
                content: "Let me check something".to_string()
            }
        );

        // Third message is the tool message converted to text
        assert_eq!(messages[2].role, MessageRole::User);
        assert_eq!(
            messages[2].content,
            MessageContent::StructuredContent {
                content: StructuredContent(vec![ContentBlock::ToolResult {
                    tool_use_id: "empty_tool".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "(No output)".to_string(),
                    }],
                    is_error: None,
                }]),
            }
        );
    }

    #[test]
    fn test_convert_conversation_with_non_tool_messages_after_tool() {
        let mut conv = Conversation::new();
        conv.add_message(AppMessage::new_text(
            conversation::Role::User,
            "Hello".to_string(),
        ));
        // Construct assistant message with ToolCall block
        conv.add_message(AppMessage::new_with_blocks(
            conversation::Role::Assistant,
            vec![AppMessageContentBlock::ToolCall(AppToolCall {
                id: "tool_call_1".to_string(),
                name: "tool_1".to_string(),
                parameters: serde_json::Value::Null,
            })],
        ));

        // Add ToolResult message
        conv.add_message(AppMessage::new_with_blocks(
            conversation::Role::Tool,
            vec![AppMessageContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                result: "Result 1".to_string(),
            }],
        ));
        conv.add_message(AppMessage::new_text(
            conversation::Role::User,
            "What about this?".to_string(),
        ));

        let messages = convert_conversation(&conv);
        println!("Non-tool messages: {:?}", messages);

        assert_eq!(messages.len(), 4);

        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(
            messages[0].content,
            MessageContent::Text {
                content: "Hello".to_string()
            }
        );

        // The assistant message with tool calls
        assert_eq!(messages[1].role, MessageRole::Assistant);
        match &messages[1].content {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 1);
                if let ContentBlock::ToolUse { id, name, input: _ } = &array[0] {
                    assert_eq!(id, "tool_call_1");
                    assert_eq!(name, "tool_1");
                } else {
                    panic!("Expected ToolUse");
                }
            }
            _ => panic!("Expected StructuredContent"),
        }

        // Tool message is preserved as a user message with readable format
        assert_eq!(messages[2].role, MessageRole::User);
        match &messages[2].content {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 1); // Expect one ToolResult block
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content: result_blocks,
                    is_error,
                } = &array[0]
                {
                    assert_eq!(tool_use_id, "tool_1");
                    assert!(is_error.is_none() || !is_error.unwrap());
                    assert_eq!(result_blocks.len(), 1);
                    if let ContentBlock::Text { text } = &result_blocks[0] {
                        assert_eq!(text, "Result 1");
                    } else {
                        panic!("Expected inner Text block");
                    }
                } else {
                    panic!("Expected ToolResult block inside StructuredContent");
                }
            }
            _ => panic!("Expected StructuredContent for tool result"),
        }

        // The subsequent user message should be included as is
        assert_eq!(messages[3].role, MessageRole::User);
        assert_eq!(
            messages[3].content,
            MessageContent::Text {
                content: "What about this?".to_string()
            }
        );
    }
}
