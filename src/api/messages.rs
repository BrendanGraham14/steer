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

    pub fn from_app_message(
        app_message: &crate::app::Message,
        next_messages: &[crate::app::Message],
    ) -> Self {
        // Extract the message content
        let content_string = app_message.content_string();
        let is_empty = content_string.trim().is_empty();
        let message_id = app_message.id.clone();

        if is_empty && app_message.role != crate::app::Role::Assistant {
            // Log warning about empty content
            crate::utils::logging::warn(
                "message.from_app_message",
                &format!(
                    "Found empty content for role {:?}, substituting placeholder",
                    app_message.role
                ),
            );

            // Return placeholder content for non-assistant empty messages
            return Self {
                role: "user".to_string(),
                content_type: MessageContent::Text {
                    content: "Message with no content".to_string(),
                },
                id: Some(message_id),
            };
        }

        match app_message.role {
            crate::app::Role::System => Self {
                role: "system".to_string(),
                content_type: MessageContent::Text {
                    content: content_string,
                },
                id: Some(message_id),
            },
            crate::app::Role::User => {
                // Check if this is a special tool result message
                match &app_message.content {
                    crate::app::conversation::MessageContent::ToolResult {
                        tool_use_id,
                        result,
                    } => {
                        return Self::new_tool_result_with_id(
                            tool_use_id,
                            result.clone(),
                            message_id,
                        );
                    }
                    _ => {}
                }

                // Regular user message
                Message {
                    role: "user".to_string(),
                    content_type: MessageContent::Text {
                        content: content_string,
                    },
                    id: Some(message_id),
                }
            }
            crate::app::Role::Assistant => {
                // Check for tool calls in this message
                match &app_message.content {
                    crate::app::conversation::MessageContent::ToolCalls(tool_calls) => {
                        // Convert tool calls to our strongly typed ContentBlock format
                        let mut content_blocks = Vec::new();

                        for tool_call in tool_calls {
                            content_blocks.push(ContentBlock::ToolUse {
                                id: tool_call.id.clone(),
                                name: tool_call.name.clone(),
                                input: tool_call.parameters.clone(),
                            });
                        }

                        return Message {
                            role: "assistant".to_string(),
                            content_type: MessageContent::StructuredContent {
                                content: StructuredContent(content_blocks),
                            },
                            id: Some(message_id),
                        };
                    }
                    _ => {}
                }

                // Check if there are any tool result messages after this assistant message
                for msg in next_messages {
                    if msg.role == crate::app::Role::Tool {
                        // Check for tool result content
                        if let crate::app::conversation::MessageContent::ToolResult {
                            tool_use_id,
                            result,
                        } = &msg.content
                        {
                            // Create a properly typed tool result content block
                            let content = StructuredContent(vec![ContentBlock::ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                content: result.clone(),
                            }]);

                            return Message {
                                role: "user".to_string(),
                                content_type: MessageContent::StructuredContent { content },
                                id: Some(message_id),
                            };
                        }
                    } else {
                        // Stop looking for tool messages once we hit a non-tool message
                        break;
                    }
                }

                // If no tool results found, return regular assistant message
                Message {
                    role: "assistant".to_string(),
                    content_type: MessageContent::Text {
                        content: content_string,
                    },
                    id: Some(message_id),
                }
            }
            crate::app::Role::Tool => {
                // Regular tool messages - displayed to user but not sent to API
                // Convert to a user message with plain text
                Message {
                    role: "user".to_string(),
                    content_type: MessageContent::Text {
                        content: content_string,
                    },
                    id: Some(message_id),
                }
            }
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
    let mut messages = Vec::new();
    let mut system_content = None;

    for (i, msg) in conversation.messages.iter().enumerate() {
        match msg.role {
            crate::app::Role::System => {
                // Store system message content
                let content_string = msg.content_string();
                if !content_string.trim().is_empty() {
                    if let crate::app::conversation::MessageContent::Text(content) = &msg.content {
                        system_content = Some(content.clone());
                    }
                }
            }
            _ => {
                // Get content string to check if empty
                let content_string = msg.content_string();

                // Skip empty messages except for assistants (Claude API allows empty assistant messages)
                if !content_string.trim().is_empty() || msg.role == crate::app::Role::Assistant {
                    // Get remaining messages for look-ahead
                    let remaining_messages = &conversation.messages[i + 1..];

                    // Add message types to the messages array
                    let api_message = Message::from_app_message(msg, remaining_messages);

                    // Check the content isn't empty (except for assistant messages)
                    let is_empty = match &api_message.content_type {
                        MessageContent::Text { content } => content.trim().is_empty(),
                        MessageContent::StructuredContent { content } => content.0.is_empty(),
                    };

                    if !is_empty || api_message.role == "assistant" {
                        messages.push(api_message);
                    } else {
                        crate::utils::logging::warn(
                            "messages.convert_conversation",
                            &format!(
                                "Skipping empty content message with role {}",
                                api_message.role
                            ),
                        );
                    }
                }
            }
        }
    }

    // Ensure we don't end with an empty message that's not an assistant
    if let Some(last) = messages.last() {
        let is_empty = match &last.content_type {
            MessageContent::Text { content } => content.trim().is_empty(),
            MessageContent::StructuredContent { content } => content.0.is_empty(),
        };

        if is_empty && last.role != "assistant" {
            crate::utils::logging::warn(
                "messages.convert_conversation",
                "Last message had empty content and wasn't from the assistant, removing it",
            );
            messages.pop();
        }
    }

    (messages, system_content)
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
        assert_eq!(messages[1].role, "user");
        match &messages[1].content_type {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 1);
                if let ContentBlock::ToolResult { tool_use_id, content } = &array[0] {
                    assert_eq!(tool_use_id, "tool_1");
                    assert_eq!(content, "Result 1");
                } else {
                    panic!("Expected ToolResult");
                }
            }
            _ => panic!("Expected StructuredContent"),
        }

        // The tool message is converted to a user message with text content
        assert_eq!(messages[2].role, "user");
        match &messages[2].content_type {
            MessageContent::Text { content } => {
                assert!(content.contains("Tool Result from"));
                assert!(content.contains("tool_1"));
                assert!(content.contains("Result 1"));
            }
            _ => panic!("Expected Text content"),
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

        assert_eq!(messages.len(), 4); // Now expecting 4 messages (user, assistant, and 2 tool results)
        assert!(system.is_none());

        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].get_content_string(), "Hello");

        // The assistant message is converted to a user message with structured content for the first tool
        assert_eq!(messages[1].role, "user");
        match &messages[1].content_type {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 1);

                if let ContentBlock::ToolResult { tool_use_id, content } = &array[0] {
                    assert_eq!(tool_use_id, "tool_1");
                    assert_eq!(content, "Result 1");
                } else {
                    panic!("Expected ToolResult");
                }
            }
            _ => panic!("Expected StructuredContent"),
        }

        // Two tool results are each converted to user messages with human-readable format
        assert_eq!(messages[2].role, "user");
        match &messages[2].content_type {
            MessageContent::Text { content } => {
                assert!(content.contains("Tool Result from"));
                assert!(content.contains("tool_1"));
            }
            _ => panic!("Expected Text content"),
        }

        assert_eq!(messages[3].role, "user");
        match &messages[3].content_type {
            MessageContent::Text { content } => {
                assert!(content.contains("Tool Result from"));
                assert!(content.contains("tool_2"));
            }
            _ => panic!("Expected Text content"),
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
        assert_eq!(messages[1].role, "user");
        match &messages[1].content_type {
            MessageContent::StructuredContent { content } => {
                let array = &content.0;
                assert_eq!(array.len(), 1);
                if let ContentBlock::ToolResult { tool_use_id, content } = &array[0] {
                    assert_eq!(tool_use_id, "empty_tool");
                    assert_eq!(content, "");
                } else {
                    panic!("Expected ToolResult");
                }
            }
            _ => panic!("Expected StructuredContent"),
        }

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
            MessageContent::Text { content } => {
                assert!(content.contains("Tool Result from"));
                assert!(content.contains("tool_1"));
                assert!(content.contains("Result 1"));
            }
            _ => panic!("Expected Text content"),
        }

        // The subsequent user message should be included as is
        assert_eq!(messages[3].role, "user");
        assert_eq!(messages[3].get_content_string(), "What about this?");
    }
}
