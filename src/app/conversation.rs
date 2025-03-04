use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Role in the conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub timestamp: u64,
    pub id: String,
}

impl Message {
    pub fn new(role: Role, content: String) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs();
        
        let id = format!("msg_{}", timestamp);
        
        Self {
            role,
            content,
            timestamp,
            id,
        }
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
    
    pub fn clear(&mut self) {
        self.messages.clear();
    }
    
    pub fn add_system_message(&mut self, content: String) {
        self.add_message(Role::System, content);
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
                
                serde_json::json!({
                    "role": role_str,
                    "content": msg.content
                })
            })
            .collect()
    }
    
    /// Compact the conversation by summarizing older messages
    pub async fn compact(&mut self, api_client: &crate::api::Client) -> anyhow::Result<()> {
        // Skip if we don't have enough messages to compact
        if self.messages.len() < 10 {
            return Ok(());
        }
        
        // Take the first N messages and ask Claude to summarize them
        let messages_to_compact: Vec<&Message> = self.messages.iter().take(self.messages.len() - 5).collect();
        
        let summary_prompt = format!(
            "Summarize the following conversation concisely, preserving key information that would be needed to continue the conversation. Focus on code-related details, decisions, and context:\n\n{}",
            messages_to_compact
                .iter()
                .map(|msg| format!("{}: {}", msg.role, msg.content))
                .collect::<Vec<_>>()
                .join("\n\n")
        );
        
        // Call the API to get a summary
        let summary = api_client.generate_summary(&summary_prompt).await?;
        
        // Replace the compacted messages with a single system message containing the summary
        let new_messages = Vec::from_iter(self.messages.drain(messages_to_compact.len()..));
        self.messages.clear();
        self.add_system_message(format!("Previous conversation summary:\n{}", summary));
        self.messages.extend(new_messages);
        
        Ok(())
    }
}