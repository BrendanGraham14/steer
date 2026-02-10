//! Message types for conversation representation.
//!
//! This module contains the core message types used throughout the application:
//! - `Message` - The main message struct with metadata
//! - `MessageData` - Role-specific content (User, Assistant, Tool)
//! - Content types for each role

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use steer_tools::ToolCall;
pub use steer_tools::result::ToolResult;
use strum_macros::Display;

/// Role in the conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Copy, Display)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ImageSource {
    SessionFile { relative_path: String },
    DataUrl { data_url: String },
    Url { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageContent {
    pub mime_type: String,
    pub source: ImageSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Content that can be sent by a user
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContent {
    Text {
        text: String,
    },
    Image {
        image: ImageContent,
    },
    CommandExecution {
        command: String,
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
}

impl UserContent {
    pub fn format_command_execution_as_xml(
        command: &str,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
    ) -> String {
        format!(
            r"<executed_command>
    <command>{command}</command>
    <stdout>{stdout}</stdout>
    <stderr>{stderr}</stderr>
    <exit_code>{exit_code}</exit_code>
</executed_command>"
        )
    }
}

/// Different types of thought content from AI models
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "thought_type")]
pub enum ThoughtContent {
    /// Simple thought content (e.g., from Gemini)
    #[serde(rename = "simple")]
    Simple { text: String },
    /// Claude-style thinking with signature
    #[serde(rename = "signed")]
    Signed { text: String, signature: String },
    /// Claude-style redacted thinking
    #[serde(rename = "redacted")]
    Redacted { data: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct ThoughtSignature(String);

impl ThoughtSignature {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ThoughtContent {
    /// Extract displayable text from any thought type
    pub fn display_text(&self) -> String {
        match self {
            ThoughtContent::Simple { text } => text.clone(),
            ThoughtContent::Signed { text, .. } => text.clone(),
            ThoughtContent::Redacted { .. } => "[Redacted Thinking]".to_string(),
        }
    }
}

/// Content that can be sent by an assistant
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Text {
        text: String,
    },
    Image {
        image: ImageContent,
    },
    ToolCall {
        tool_call: ToolCall,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<ThoughtSignature>,
    },
    Thought {
        thought: ThoughtContent,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub timestamp: u64,
    pub id: String,
    pub parent_message_id: Option<String>,
    pub data: MessageData,
}

/// A message in the conversation, with role-specific content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum MessageData {
    User {
        content: Vec<UserContent>,
    },
    Assistant {
        content: Vec<AssistantContent>,
    },
    Tool {
        tool_use_id: String,
        result: ToolResult,
    },
}

impl Message {
    pub fn role(&self) -> Role {
        match &self.data {
            MessageData::User { .. } => Role::User,
            MessageData::Assistant { .. } => Role::Assistant,
            MessageData::Tool { .. } => Role::Tool,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    pub fn parent_message_id(&self) -> Option<&str> {
        self.parent_message_id.as_deref()
    }

    /// Helper to get current timestamp
    pub fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Helper to generate unique IDs
    pub fn generate_id(prefix: &str, _timestamp: u64) -> String {
        use uuid::Uuid;
        format!("{}_{}", prefix, Uuid::now_v7())
    }

    /// Extract text content from the message
    pub fn extract_text(&self) -> String {
        match &self.data {
            MessageData::User { content } => content
                .iter()
                .map(|c| match c {
                    UserContent::Text { text } => text.clone(),
                    UserContent::Image { .. } => "[Image]".to_string(),
                    UserContent::CommandExecution { stdout, .. } => stdout.clone(),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            MessageData::Assistant { content } => content
                .iter()
                .map(|c| match c {
                    AssistantContent::Text { text } => text.clone(),
                    AssistantContent::Image { .. } => "[Image]".to_string(),
                    AssistantContent::ToolCall { .. } | AssistantContent::Thought { .. } => {
                        String::new()
                    }
                })
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            MessageData::Tool { result, .. } => result.llm_format(),
        }
    }

    /// Get a string representation of the message content
    pub fn content_string(&self) -> String {
        match &self.data {
            MessageData::User { content } => content
                .iter()
                .map(|c| match c {
                    UserContent::Text { text } => text.clone(),
                    UserContent::Image { image } => {
                        format!("[Image: {}]", image.mime_type)
                    }
                    UserContent::CommandExecution {
                        command,
                        stdout,
                        stderr,
                        exit_code,
                    } => {
                        let mut output = format!("$ {command}\n{stdout}");
                        if *exit_code != 0 {
                            output.push_str(&format!("\nExit code: {exit_code}"));
                        }
                        if !stderr.is_empty() {
                            output.push_str(&format!("\nError: {stderr}"));
                        }
                        output
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            MessageData::Assistant { content } => content
                .iter()
                .map(|c| match c {
                    AssistantContent::Text { text } => text.clone(),
                    AssistantContent::Image { image } => {
                        format!("[Image: {}]", image.mime_type)
                    }
                    AssistantContent::ToolCall { tool_call, .. } => {
                        format!("[Tool Call: {}]", tool_call.name)
                    }
                    AssistantContent::Thought { thought } => {
                        format!("[Thought: {}]", thought.display_text())
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            MessageData::Tool { result, .. } => {
                // This is a simplified representation. The TUI will have a more detailed view.
                let result_type = match result {
                    ToolResult::Search(_) => "Search Result",
                    ToolResult::FileList(_) => "File List",
                    ToolResult::FileContent(_) => "File Content",
                    ToolResult::Edit(_) => "Edit Result",
                    ToolResult::Bash(_) => "Bash Result",
                    ToolResult::Glob(_) => "Glob Result",
                    ToolResult::TodoRead(_) => "Todo List",
                    ToolResult::TodoWrite(_) => "Todo Update",
                    ToolResult::Fetch(_) => "Fetch Result",
                    ToolResult::Agent(_) => "Agent Result",
                    ToolResult::External(_) => "External Tool Result",
                    ToolResult::Error(_) => "Error",
                };
                format!("[Tool Result: {result_type}]")
            }
        }
    }
}
