use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use serde_json::Value;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub parameters: Value,
}

/// Content types for messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageContent {
    Text(String),
    ToolCalls(Vec<ToolCall>),
    ToolResult {
        tool_use_id: String,
        result: String,
    },
}

impl MessageContent {
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(content) => content.clone(),
            Self::ToolCalls(tool_calls) => {
                format!("<tool_calls>{}</tool_calls>", serde_json::to_string(tool_calls).unwrap_or_default())
            },
            Self::ToolResult { tool_use_id, result } => {
                format!("Tool Result from {}: {}", tool_use_id, result)
            },
        }
    }
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: MessageContent,
    pub timestamp: u64,
    pub id: String,
}

impl Message {
    pub fn new(role: Role, content: String) -> Self {
        let message_content = MessageContent::Text(content);
        Self::new_with_content(role, message_content)
    }
    
    pub fn new_with_content(role: Role, content: MessageContent) -> Self {
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
            content,
            timestamp,
            id,
        }
    }
    
    pub fn content_string(&self) -> String {
        self.content.as_text()
    }
}

/// A conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn add_message(&mut self, role: Role, content: String) {
        let message = Message::new(role, content);
        self.messages.push(message);
    }
    
    pub fn add_message_with_content(&mut self, role: Role, content: MessageContent) {
        let message = Message::new_with_content(role, content);
        self.messages.push(message);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn add_system_message(&mut self, content: String) {
        self.add_message(Role::System, content);
    }
    
    pub fn add_tool_call(&mut self, tool_call: ToolCall) {
        // Check if the last message is from the assistant and already has tool calls
        if let Some(last) = self.messages.last_mut() {
            if last.role == Role::Assistant {
                match &mut last.content {
                    MessageContent::ToolCalls(tool_calls) => {
                        tool_calls.push(tool_call);
                        return;
                    }
                    MessageContent::Text(text) if text.trim().is_empty() => {
                        // Replace empty text with tool calls
                        last.content = MessageContent::ToolCalls(vec![tool_call]);
                        return;
                    }
                    _ => {}
                }
            }
        }
        
        // Otherwise, create a new assistant message with tool calls
        self.add_message_with_content(Role::Assistant, MessageContent::ToolCalls(vec![tool_call]));
    }
    
    pub fn add_tool_result(&mut self, tool_use_id: String, result: String) {
        self.add_message_with_content(
            Role::Tool, 
            MessageContent::ToolResult { 
                tool_use_id, 
                result 
            }
        );
    }

    pub fn to_api_messages(&self) -> Vec<serde_json::Value> {
        self.messages
            .iter()
            .map(|msg| {
                let role_str = match msg.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool", // Note: Anthropic API may handle tools differently
                };

                match &msg.content {
                    MessageContent::Text(content) => {
                        // Create the message JSON with text content
                        serde_json::json!({
                            "role": role_str,
                            "content": {
                                "type": "text",
                                "text": content
                            }
                        })
                    },
                    MessageContent::ToolCalls(tool_calls) => {
                        // Create message JSON with tool calls
                        serde_json::json!({
                            "role": role_str,
                            "content": tool_calls.iter().map(|tc| {
                                serde_json::json!({
                                    "type": "tool_use",
                                    "id": tc.id,
                                    "name": tc.name,
                                    "parameters": tc.parameters
                                })
                            }).collect::<Vec<_>>()
                        })
                    },
                    MessageContent::ToolResult { tool_use_id, result } => {
                        // Create message JSON with tool result
                        serde_json::json!({
                            "role": "user",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": tool_use_id,
                                "content": result
                            }]
                        })
                    }
                }
            })
            .collect()
    }

    /// Get the system prompt if present
    pub fn system_prompt(&self) -> Option<String> {
        for message in &self.messages {
            if message.role == Role::System {
                if let MessageContent::Text(content) = &message.content {
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
            content_type: messages::MessageContent::Text {
                content: summary_prompt,
            },
            id: None,
        }];

        // Call the API to get a summary
        let summary = api_client.complete(prompt_messages, None, None).await?;

        // Replace the compacted messages with a single system message containing the summary
        let new_messages = self.messages.split_off(messages_to_compact.len());
        self.messages.clear();
        self.add_system_message(format!(
            "Previous conversation summary:\n{}",
            summary.extract_text()
        ));
        self.messages.extend(new_messages);

        Ok(())
    }
}
