use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::messages;

/// Role in the conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Copy)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool, // For tool outputs
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "System"),
            Self::User => write!(f, "User"),
            Self::Assistant => write!(f, "Claude"),
            Self::Tool => write!(f, "Tool"),
        }
    }
}

/// Tool call that can be attached to assistant messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub parameters: Value,
}

/// Represents a block of content within a single message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageContentBlock {
    Text(String),
    ToolCall(ToolCall),
    ToolResult { tool_use_id: String, result: String },
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content_blocks: Vec<MessageContentBlock>,
    pub timestamp: u64,
    pub id: String,
}

impl Message {
    /// Creates a new message with simple text content.
    pub fn new_text(role: Role, content: String) -> Self {
        Self::new_with_blocks(role, vec![MessageContentBlock::Text(content)])
    }

    /// Creates a new message with a single tool call.
    pub fn new_tool_call(role: Role, tool_call: ToolCall) -> Self {
        Self::new_with_blocks(role, vec![MessageContentBlock::ToolCall(tool_call)])
    }

    /// Creates a new message with a vector of content blocks.
    pub fn new_with_blocks(role: Role, content_blocks: Vec<MessageContentBlock>) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs();

        // Use role-specific prefixes for message IDs
        // This helps avoid confusion between different message types
        let prefix = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::System => "system",
        };

        // Add a short random suffix to avoid collisions
        let random_suffix = format!("{:04x}", (timestamp % 10000));
        let id = format!("{}_{}{}", prefix, timestamp, random_suffix);

        Self {
            role,
            content_blocks,
            timestamp,
            id,
        }
    }

    /// Returns a simple string representation of the message content.
    /// Joins text blocks and provides placeholders for non-text blocks.
    pub fn content_string(&self) -> String {
        self.content_blocks
            .iter()
            .map(|block| match block {
                MessageContentBlock::Text(text) => text.clone(),
                MessageContentBlock::ToolCall(tc) => format!("[Tool Call: {}]", tc.name),
                MessageContentBlock::ToolResult { tool_use_id, .. } => {
                    format!("[Tool Result for {}]", tool_use_id)
                }
            })
            .collect::<Vec<_>>()
            .join("\n") // Join different blocks with newline for basic representation
    }

    /// Placeholder method for toggling truncation state.
    /// The actual state is managed in the TUI's FormattedMessage.
    pub fn toggle_truncation(&mut self) {
        // No-op here, state is in FormattedMessage
        crate::utils::logging::debug(
            "Message.toggle_truncation",
            &format!("Toggle truncation requested for message ID: {}", self.id),
        );
    }
}

/// A conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
    pub working_directory: PathBuf,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            working_directory: PathBuf::new(),
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn add_message_with_blocks(
        &mut self,
        role: Role,
        content_blocks: Vec<MessageContentBlock>,
    ) {
        let message = Message::new_with_blocks(role, content_blocks);
        self.messages.push(message);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn add_system_message(&mut self, content: String) {
        self.add_message(Message::new_text(Role::System, content));
    }

    pub fn add_tool_result(&mut self, tool_use_id: String, result: String) {
        self.add_message(Message::new_with_blocks(
            Role::Tool,
            vec![MessageContentBlock::ToolResult {
                tool_use_id,
                result,
            }],
        ));
    }

    /// Get the system prompt if present
    pub fn system_prompt(&self) -> Option<String> {
        for message in &self.messages {
            if message.role == Role::System {
                // Assuming system prompts are always single text blocks
                if let Some(MessageContentBlock::Text(content)) = message.content_blocks.first() {
                    return Some(content.clone());
                }
            }
        }
        None
    }

    /// Convert conversation to a string
    pub fn to_string(&self) -> String {
        self.messages
            .iter()
            .map(|msg| format!("{}: {}", msg.role, msg.content_string()))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Compact the conversation by summarizing older messages (deprecated)
    pub async fn compact(&mut self, api_client: &crate::api::Client) -> anyhow::Result<()> {
        // Skip if we don't have enough messages to compact
        if self.messages.len() < 10 {
            return Ok(());
        }

        // Create a copy of the messages to compact to avoid borrowing issues
        let messages_to_compact: Vec<Message> = self
            .messages
            .iter()
            .take(self.messages.len() - 5)
            .cloned()
            .collect();

        let summary_prompt = format!(
            "Summarize the following conversation concisely, preserving key information that would be needed to continue the conversation. Focus on code-related details, decisions, and context:\n\n{}",
            messages_to_compact
                .iter()
                .map(|msg| format!("{}: {}", msg.role, msg.content_string()))
                .collect::<Vec<_>>()
                .join("\n\n")
        );

        let prompt_messages = vec![messages::Message {
            role: "user".to_string(),
            content: messages::MessageContent::Text {
                content: summary_prompt,
            },
            id: None,
        }];

        // Call the API to get a summary
        let summary = api_client.complete(prompt_messages, None, None).await?;

        // Replace the compacted messages with a single system message containing the summary
        let new_messages = self.messages.split_off(messages_to_compact.len());
        self.messages.clear();
        self.add_message(Message::new_text(
            Role::System,
            format!("Previous conversation summary:\n{}", summary.extract_text()),
        ));
        self.messages.extend(new_messages);

        Ok(())
    }

    /// Convert internal messages to the format required by the API client
    pub fn get_messages_for_api(&self) -> Vec<crate::api::Message> {
        self.messages
            .iter()
            .filter(|msg| msg.role != Role::System) // Filter out system messages here
            .map(|msg| {
                // Convert internal Role to API Role (String)
                let api_role = match msg.role {
                    // Role::System => "system".to_string(), // System messages are handled separately
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                    Role::Tool => "user".to_string(), // Tool results are sent as user messages in the API
                    Role::System => panic!("System message should have been filtered out"), // Should not happen
                };

                // Convert content blocks
                let api_content = if msg.role == Role::Tool {
                    // Tool role messages become user role with tool_result content
                    let tool_results: Vec<messages::ContentBlock> = msg
                        .content_blocks
                        .iter()
                        .filter_map(|block| match block {
                            MessageContentBlock::ToolResult { tool_use_id, result } => {
                                let is_error = result.starts_with("Error:");
                                let result_content = if result.trim().is_empty() {
                                    "(No output)".to_string()
                                } else {
                                    result.clone()
                                };
                                Some(messages::ContentBlock::ToolResult {
                                    tool_use_id: tool_use_id.clone(),
                                    content: vec![messages::ContentBlock::Text {
                                        text: result_content,
                                    }],
                                    is_error: if is_error { Some(true) } else { None },
                                })
                            }
                            _ => {
                                crate::utils::logging::warn(
                                    "conversation.get_messages_for_api",
                                    &format!(
                                        "Unexpected content block type {:?} found in Tool message {}",
                                        block,
                                        msg.id
                                    ),
                                );
                                None
                            }
                        })
                        .collect();
                    messages::MessageContent::StructuredContent {
                        content: messages::StructuredContent(tool_results),
                    }
                } else {
                    // User or Assistant messages
                    let api_blocks: Vec<messages::ContentBlock> = msg.content_blocks.iter().map(|block| match block {
                        MessageContentBlock::Text(text) => messages::ContentBlock::Text { text: text.clone() },
                        MessageContentBlock::ToolCall(tc) => messages::ContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            input: tc.parameters.clone(),
                        },
                        MessageContentBlock::ToolResult { .. } => {
                             crate::utils::logging::error(
                                "conversation.get_messages_for_api",
                                &format!(
                                    "Unexpected ToolResult block found in {:?} message {}",
                                    msg.role,
                                    msg.id
                                ),
                            );
                            // Return a placeholder or skip? Let's return a text placeholder for now.
                            messages::ContentBlock::Text { text: "[Internal Error: Unexpected ToolResult]".to_string() }
                        }
                    }).collect();

                    // Determine if we need StructuredContent or simple Text
                    if api_blocks.len() == 1 {
                        if let Some(messages::ContentBlock::Text { text }) = api_blocks.first() {
                            // Single text block -> Simple Text content
                             messages::MessageContent::Text { content: text.clone() }
                        } else {
                            // Single non-text block -> Structured Content
                            messages::MessageContent::StructuredContent {
                                content: messages::StructuredContent(api_blocks),
                            }
                        }
                    } else {
                         // Multiple blocks -> Structured Content
                         messages::MessageContent::StructuredContent {
                            content: messages::StructuredContent(api_blocks),
                        }
                    }
                };

                crate::api::Message {
                    role: api_role,
                    content: api_content,
                    id: Some(msg.id.clone()),
                }
            })
            .collect()
    }

    /// Create the prompt for summarizing the conversation
    pub fn create_summary_prompt(&self) -> String {
        // Use all messages except the last few (e.g., keep last 5)
        let messages_to_summarize = self
            .messages
            .iter()
            .take(self.messages.len().saturating_sub(5));
        let conversation_text = messages_to_summarize
            .map(|msg| format!("{}: {}", msg.role, msg.content_string()))
            .collect::<Vec<_>>()
            .join("\n\n");

        if conversation_text.is_empty() {
            return String::new();
        }

        format!(
            "Summarize the following conversation concisely, preserving key information that would be needed to continue the conversation. Focus on code-related details, decisions, and context:\n\n{}",
            conversation_text
        )
    }

    /// Clear all messages except the system prompt (if one exists)
    pub fn clear_except_system(&mut self) {
        let system_message = self
            .messages
            .iter()
            .find(|m| m.role == Role::System)
            .cloned();
        self.messages.clear();
        if let Some(msg) = system_message {
            self.messages.push(msg);
        }
    }
}
