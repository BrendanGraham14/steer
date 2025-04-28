use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::messages;
use tokio_util::sync::CancellationToken;

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

    pub fn clear(&mut self) {
        self.messages.clear();
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

    /// Compact the conversation by summarizing older messages
    pub async fn compact(
        &mut self,
        api_client: &crate::api::Client,
        token: CancellationToken,
    ) -> anyhow::Result<()> {
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

        // Don't create a new token, use the passed one
        // let token = CancellationToken::new();

        let summary = api_client
            .complete(prompt_messages, None, None, token.clone()) // Pass the token
            .await?;
        let summary_text = summary.extract_text();

        // Replace the compacted messages with a single system message containing the summary
        let new_messages = self.messages.split_off(messages_to_compact.len());
        self.messages.clear();
        self.add_message(Message::new_text(
            Role::System,
            format!("Previous conversation summary:\n{}", summary_text),
        ));
        self.messages.extend(new_messages);

        Ok(())
    }
}
