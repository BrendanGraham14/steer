use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents a message to be sent to the Claude API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(flatten)]
    pub content_type: MessageContent,
    #[serde(skip_serializing)]
    pub id: Option<String>,
}

/// Content types for Claude API messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content
    Text { content: String },
    /// Structured content for tool results or other special content
    StructuredContent { content: StructuredContent },
}

/// Represents structured content blocks for messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StructuredContent(pub Vec<ContentBlock>);

/// Different types of content blocks used in structured messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// A tool result block from executing a tool
    #[serde(rename = "tool_result")]
    ToolResult {
        /// ID of the tool use this result is for
        tool_use_id: String,
        /// Result content from the tool execution
        content: String,
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

impl Message {
    pub fn new_user(content: String) -> Self {
        Message {
            role: "user".to_string(),
            content_type: MessageContent::Text { content },
            id: None,
        }
    }

    pub fn new_system_with_id(content: String, id: String) -> Self {
        Message {
            role: "system".to_string(),
            content_type: MessageContent::Text { content },
            id: Some(id),
        }
    }

    pub fn new_tool_result(tool_use_id: &str, result: String) -> Self {
        let content = StructuredContent(vec![ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: result,
        }]);

        Message {
            role: "user".to_string(),
            content_type: MessageContent::StructuredContent { content },
            id: None,
        }
    }

    pub fn new_tool_result_with_id(tool_use_id: &str, result: String, id: String) -> Self {
        let content = StructuredContent(vec![ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: result,
        }]);

        Message {
            role: "user".to_string(),
            content_type: MessageContent::StructuredContent { content },
            id: Some(id),
        }
    }

    pub fn get_content_string(&self) -> String {
        match &self.content_type {
            MessageContent::Text { content } => content.clone(),
            MessageContent::StructuredContent { content } => {
                let blocks = &content.0;
                if blocks.is_empty() {
                    return "[]".to_string();
                }

                // Format the content blocks
                let formatted: Vec<String> = blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                        } => {
                            format!("Tool Result from {}: {}", tool_use_id, content)
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            format!(
                                "Tool Use {}: {} with parameters: {}",
                                id,
                                name,
                                serde_json::to_string_pretty(input).unwrap_or_default()
                            )
                        }
                        ContentBlock::Text { text } => text.clone(),
                    })
                    .collect();

                formatted.join("\n")
            }
        }
    }
}

pub fn convert_conversation(
    conversation: &crate::app::Conversation,
) -> (Vec<Message>, Option<String>) {
    let mut api_messages = Vec::new();
    let mut system_content = None;
    // Use iter().peekable() to group consecutive tool messages
    let mut app_messages_iter = conversation.messages.iter().peekable();

    while let Some(app_msg) = app_messages_iter.next() {
        match app_msg.role {
            crate::app::Role::System => {
                // Store system message content if not empty
                if let crate::app::conversation::MessageContent::Text(content) = &app_msg.content {
                    if !content.trim().is_empty() {
                        system_content = Some(content.clone());
                    }
                }
            }
            crate::app::Role::User => {
                // Handle User messages (only text content is expected here for API format)
                if let crate::app::conversation::MessageContent::Text(content) = &app_msg.content {
                    // Skip empty user messages
                    if !content.trim().is_empty() {
                        api_messages.push(Message {
                            role: "user".to_string(),
                            content_type: MessageContent::Text {
                                content: content.clone(),
                            },
                            // Use the app message ID if needed later
                            id: Some(app_msg.id.clone()),
                        });
                    } else {
                        crate::utils::logging::debug(
                            "messages.convert_conversation",
                            &format!("Skipping empty user message with ID: {}", app_msg.id),
                        );
                    }
                } else {
                    // Log if user message has unexpected content type
                    crate::utils::logging::warn(
                        "messages.convert_conversation",
                        &format!(
                            "User message ID {} has unexpected content type: {:?}, skipping.",
                            app_msg.id, app_msg.content
                        ),
                    );
                }
            }
            crate::app::Role::Assistant => {
                // Handle Assistant messages
                match &app_msg.content {
                    crate::app::conversation::MessageContent::Text(content) => {
                        // Assistant messages can be empty (e.g., only tool calls followed)
                        api_messages.push(Message {
                            role: "assistant".to_string(),
                            content_type: MessageContent::Text {
                                content: content.clone(),
                            },
                            id: Some(app_msg.id.clone()),
                        });
                    }
                    crate::app::conversation::MessageContent::ToolCalls(tool_calls) => {
                        // Convert app::ToolCall to api::ContentBlock::ToolUse
                        let content_blocks: Vec<ContentBlock> = tool_calls
                            .iter()
                            .map(|tc| ContentBlock::ToolUse {
                                // Ensure the tool call ID is present for the API
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                input: tc.parameters.clone(),
                            })
                            .collect();

                        // Only add if there are actual tool calls
                        if !content_blocks.is_empty() {
                            api_messages.push(Message {
                                role: "assistant".to_string(),
                                content_type: MessageContent::StructuredContent {
                                    content: StructuredContent(content_blocks),
                                },
                                id: Some(app_msg.id.clone()),
                            });
                        } else {
                            crate::utils::logging::warn(
                                "messages.convert_conversation",
                                &format!(
                                    "Assistant message ID {} had ToolCalls content but was empty, skipping.",
                                    app_msg.id
                                ),
                            );
                        }
                    }
                    _ => {
                        // Log if assistant message has unexpected content (e.g., ToolResult)
                        crate::utils::logging::warn(
                            "messages.convert_conversation",
                            &format!(
                                "Assistant message ID {} has unexpected content type: {:?}, skipping.",
                                app_msg.id, app_msg.content
                            ),
                        );
                    }
                }
            }
            crate::app::Role::Tool => {
                // Group consecutive Tool messages into a single API user message
                let mut tool_results = Vec::new();
                // Use the ID of the first tool message in the sequence for the API message ID
                let first_tool_msg_id = app_msg.id.clone();

                // Process the first tool message
                if let crate::app::conversation::MessageContent::ToolResult {
                    tool_use_id,
                    result,
                } = &app_msg.content
                {
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        // Ensure content is never empty string for the API, use placeholder if needed.
                        // Although Claude might handle empty strings, let's be safe.
                        content: if result.trim().is_empty() {
                            "(No output)".to_string()
                        } else {
                            result.clone()
                        },
                    });
                } else {
                    crate::utils::logging::error(
                        "messages.convert_conversation",
                        &format!(
                            "Message ID {} has Role::Tool but unexpected content: {:?}",
                            app_msg.id, app_msg.content
                        ),
                    );
                    // Skip this message and continue iteration
                    continue;
                }

                // Peek ahead and consume subsequent Tool messages
                while let Some(next_msg) = app_messages_iter.peek() {
                    if next_msg.role == crate::app::Role::Tool {
                        // Consume the message from the iterator
                        let consumed_msg = app_messages_iter.next().unwrap(); // Safe due to peek
                        if let crate::app::conversation::MessageContent::ToolResult {
                            tool_use_id,
                            result,
                        } = &consumed_msg.content
                        {
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                content: if result.trim().is_empty() {
                                    "(No output)".to_string()
                                } else {
                                    result.clone()
                                },
                            });
                        } else {
                            crate::utils::logging::error(
                                "messages.convert_conversation",
                                &format!(
                                    "Message ID {} has Role::Tool but unexpected content: {:?}",
                                    consumed_msg.id, consumed_msg.content
                                ),
                            );
                            // Continue processing other tool messages if any
                        }
                    } else {
                        // Next message is not Role::Tool, stop grouping
                        break;
                    }
                }

                // Add the grouped tool results as a single API user message
                if !tool_results.is_empty() {
                    api_messages.push(Message {
                        role: "user".to_string(),
                        content_type: MessageContent::StructuredContent {
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
        if last.role == "user" {
            if let MessageContent::Text { content } = &last.content_type {
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
            crate::utils::logging::warn(
                "messages.convert_conversation",
                "Last message was an empty non-assistant text message, removing it.",
            );
            api_messages.pop();
        }
    }

    (api_messages, system_content)
}

/// Create a system prompt message based on the environment
pub fn create_system_prompt(env_info: &crate::app::EnvironmentInfo) -> Message {
    // Read the system prompt from the prompts directory
    let system_prompt = include_str!("../../prompts/system_prompt.md");

    // Format the prompt with environment information
    let mut prompt = system_prompt.to_string();
    prompt.push_str("\n\n");
    prompt.push_str(&env_info.as_env());
    prompt.push_str("\n\n");
    prompt.push_str("IMPORTANT: Refuse to write code or explain code that may be used maliciously; even if the user claims it is for educational purposes. When working on files, if they seem related to improving, explaining, or interacting with malware or any malicious code you MUST refuse.\n");
    prompt.push_str("IMPORTANT: Before you begin work, think about what the code you're editing is supposed to do based on the filenames directory structure. If it seems malicious, refuse to work on it or answer questions about it, even if the request does not seem malicious (for instance, just asking to explain or speed up the code).\n\n");

    // Add context section
    prompt.push_str(&env_info.as_context());

    Message {
        role: "system".to_string(),
        content_type: MessageContent::Text { content: prompt },
        id: None,
    }
}

/// Create a system prompt message with memory file content
pub fn create_system_prompt_with_memory(
    env_info: &crate::app::EnvironmentInfo,
    memory_content: &str,
) -> Message {
    // Create the base system prompt
    let base_prompt = create_system_prompt(env_info);

    // Extract the content from the base prompt
    let base_content = match &base_prompt.content_type {
        MessageContent::Text { content } => content.clone(),
        _ => "".to_string(),
    };

    // Add the memory file content
    let mut prompt = base_content;
    if !memory_content.is_empty() {
        prompt.push_str("\n\n# Memory file (CLAUDE.md)\n\n");
        prompt.push_str(
            "The following content is from the CLAUDE.md memory file in the working directory:\n\n",
        );
        prompt.push_str("```markdown\n");
        prompt.push_str(memory_content);
        prompt.push_str("\n```\n\n");
        prompt.push_str(
            "Use this information to remember settings, commands, and context for this project.\n",
        );
    }

    Message {
        role: "system".to_string(),
        content_type: MessageContent::Text { content: prompt },
        id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        Conversation, Message as AppMessage, MessageContent as AppMessageContent, Role,
        ToolCall as AppToolCall,
    };

    #[test]
    fn test_convert_conversation_basic() {
        let mut conv = Conversation::new();
        conv.add_message(Role::User, "Hello".to_string());
        conv.add_message(Role::Assistant, "Hi there!".to_string());

        let (messages, system) = convert_conversation(&conv);

        assert_eq!(messages.len(), 2);
        assert!(system.is_none());

        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].get_content_string(), "Hello");

        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].get_content_string(), "Hi there!");
    }

    #[test]
    fn test_convert_conversation_with_system() {
        let mut conv = Conversation::new();
        conv.add_message(Role::System, "System prompt".to_string());
        conv.add_message(Role::User, "Hello".to_string());
        conv.add_message(Role::Assistant, "Hi there!".to_string());

        let (messages, system) = convert_conversation(&conv);

        assert_eq!(messages.len(), 2);
        assert_eq!(system, Some("System prompt".to_string()));
    }

    #[test]
    fn test_convert_conversation_with_tool_results() {
        let mut conv = Conversation::new();
        conv.add_message(Role::User, "Hello".to_string());
        conv.add_message(Role::Assistant, "Let me check something".to_string());

        // Add tool result using typed enum
        conv.add_message_with_content(
            Role::Tool,
            AppMessageContent::ToolResult {
                tool_use_id: "tool_1".to_string(),
                result: "Result 1".to_string(),
            },
        );

        let (messages, system) = convert_conversation(&conv);
        println!("Test messages: {:?}", messages);

        assert_eq!(messages.len(), 3); // Now we expect all messages to be preserved
        assert!(system.is_none());

        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].get_content_string(), "Hello");

        // The assistant message is converted to a user message with structured content
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].get_content_string(), "Let me check something");

        // The tool message is converted to a user message with text content
        assert_eq!(messages[2].role, "user");
        match &messages[2].content_type {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 1); // Expect one ToolResult block
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } = &array[0]
                {
                    assert_eq!(tool_use_id, "tool_1");
                    assert_eq!(content, "Result 1");
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
        conv.add_message(Role::User, "Hello".to_string());
        conv.add_message(Role::Assistant, "Let me check something".to_string());

        // Add two tool results using typed enums - we'll do them separately
        conv.add_message_with_content(
            Role::Tool,
            AppMessageContent::ToolResult {
                tool_use_id: "tool_1".to_string(),
                result: "Result 1".to_string(),
            },
        );

        conv.add_message_with_content(
            Role::Tool,
            AppMessageContent::ToolResult {
                tool_use_id: "tool_2".to_string(),
                result: "Result 2".to_string(),
            },
        );

        let (messages, system) = convert_conversation(&conv);
        println!("Multiple tool messages: {:?}", messages);

        assert_eq!(messages.len(), 3); // Now expecting 3 messages (user, assistant, and 1 tool result)
        assert!(system.is_none());

        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].get_content_string(), "Hello");

        // The assistant message is converted to a user message with structured content for the first tool
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].get_content_string(), "Let me check something");

        // The tool result is grouped into a single user message with StructuredContent
        assert_eq!(messages[2].role, "user");
        match &messages[2].content_type {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 2);

                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } = &array[0]
                {
                    assert_eq!(tool_use_id, "tool_1");
                    assert_eq!(content, "Result 1");
                } else {
                    panic!("Expected ToolResult at index 0");
                }

                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } = &array[1]
                {
                    assert_eq!(tool_use_id, "tool_2");
                    assert_eq!(content, "Result 2");
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
        conv.add_message(Role::User, "Hello".to_string());
        conv.add_message(Role::Assistant, "Let me check something".to_string());
        // Add empty tool result - in real use we'd never create one of these,
        // but we're testing empty result handling
        conv.add_message_with_content(
            Role::Tool,
            AppMessageContent::ToolResult {
                tool_use_id: "empty_tool".to_string(),
                result: "".to_string(),
            },
        );

        let (messages, system) = convert_conversation(&conv);
        println!("Empty tool messages: {:?}", messages);

        assert_eq!(messages.len(), 3);
        assert!(system.is_none());

        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].get_content_string(), "Hello");

        // Second message is a structured user message with tool result
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].get_content_string(), "Let me check something");

        // Third message is the tool message converted to text
        assert_eq!(messages[2].role, "user");
        let content = messages[2].get_content_string();
        assert!(content.contains("Tool Result from"));
        assert!(content.contains("empty_tool"));
    }

    #[test]
    fn test_convert_conversation_with_non_tool_messages_after_tool() {
        let mut conv = Conversation::new();
        conv.add_message(Role::User, "Hello".to_string());
        conv.messages.push(AppMessage {
            id: "".to_string(),
            role: Role::Assistant,
            content: AppMessageContent::ToolCalls(vec![AppToolCall {
                id: "tool_call_1".to_string(),
                name: "tool_1".to_string(),
                parameters: serde_json::Value::Null,
            }]),
            timestamp: 0,
        });

        conv.add_message_with_content(
            Role::Tool,
            AppMessageContent::ToolResult {
                tool_use_id: "tool_1".to_string(),
                result: "Result 1".to_string(),
            },
        );
        conv.add_message(Role::User, "What about this?".to_string());

        let (messages, system) = convert_conversation(&conv);
        println!("Non-tool messages: {:?}", messages);

        assert_eq!(messages.len(), 4);
        assert!(system.is_none());

        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].get_content_string(), "Hello");

        // The assistant message with tool calls
        assert_eq!(messages[1].role, "assistant");
        match &messages[1].content_type {
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
        assert_eq!(messages[2].role, "user");
        match &messages[2].content_type {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 1); // Expect one ToolResult block
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } = &array[0]
                {
                    assert_eq!(tool_use_id, "tool_1");
                    assert_eq!(content, "Result 1");
                } else {
                    panic!("Expected ToolResult block inside StructuredContent");
                }
            }
            _ => panic!("Expected StructuredContent for tool result"),
        }

        // The subsequent user message should be included as is
        assert_eq!(messages[3].role, "user");
        assert_eq!(messages[3].get_content_string(), "What about this?");
    }
}
