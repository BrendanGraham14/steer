use crate::api::Client as ApiClient;
use crate::api::Model;
use conductor_tools::ToolCall;
pub use conductor_tools::result::ToolResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

use strum_macros::Display;
use tokio_util::sync::CancellationToken;

/// Result of a conversation compaction operation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "result_type", rename_all = "snake_case")]
pub enum CompactResult {
    /// Compaction completed successfully with the summary
    Success(String),
    /// Compaction was cancelled by the user
    Cancelled,
    /// Not enough messages to compact
    InsufficientMessages,
}

/// Response from executing an app command
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "response_type", rename_all = "snake_case")]
pub enum CommandResponse {
    /// Simple text response
    Text(String),
    /// Compact command response with structured result
    Compact(CompactResult),
}

/// Types of app commands that can be executed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "command_type", rename_all = "snake_case")]
pub enum AppCommandType {
    /// Model management - list or change models
    Model { target: Option<String> },
    /// Clear the conversation
    Clear,
    /// Compact the conversation
    Compact,
}

impl AppCommandType {
    /// Parse a command string into an AppCommandType
    pub fn parse(input: &str) -> Result<Self, SlashCommandError> {
        // Trim whitespace and remove leading slash if present
        let command = input.trim();
        let command = command.strip_prefix('/').unwrap_or(command);

        // Split to get command name and args
        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            return Err(SlashCommandError::InvalidFormat(
                "Empty command".to_string(),
            ));
        }

        match parts[0] {
            "model" => {
                let target = if parts.len() > 1 {
                    Some(parts[1..].join(" "))
                } else {
                    None
                };
                Ok(AppCommandType::Model { target })
            }
            "clear" => Ok(AppCommandType::Clear),
            "compact" => Ok(AppCommandType::Compact),
            cmd => Err(SlashCommandError::UnknownCommand(cmd.to_string())),
        }
    }

    /// Get the command string representation
    pub fn as_command_str(&self) -> String {
        match self {
            AppCommandType::Model { target } => {
                if let Some(model) = target {
                    format!("model {model}")
                } else {
                    "model".to_string()
                }
            }
            AppCommandType::Clear => "clear".to_string(),
            AppCommandType::Compact => "compact".to_string(),
        }
    }
}

impl fmt::Display for AppCommandType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.as_command_str())
    }
}

impl FromStr for AppCommandType {
    type Err = SlashCommandError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Errors that can occur when parsing slash commands
#[derive(Debug, thiserror::Error)]
pub enum SlashCommandError {
    #[error("Unknown command: {0}")]
    UnknownCommand(String),
    #[error("Invalid command format: {0}")]
    InvalidFormat(String),
}

/// Role in the conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Copy, Display)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

/// Content that can be sent by a user
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContent {
    Text {
        text: String,
    },
    CommandExecution {
        command: String,
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    AppCommand {
        command: AppCommandType,
        response: Option<CommandResponse>,
    },
    // TODO: support attachments
}

impl UserContent {
    pub fn format_command_execution_as_xml(
        command: &str,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
    ) -> String {
        format!(
            r#"<executed_command>
    <command>{command}</command>
    <stdout>{stdout}</stdout>
    <stderr>{stderr}</stderr>
    <exit_code>{exit_code}</exit_code>
</executed_command>"#
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
    Text { text: String },
    ToolCall { tool_call: ToolCall },
    Thought { thought: ThoughtContent },
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
            .expect("Time went backwards")
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
                .filter_map(|c| match c {
                    UserContent::Text { text } => Some(text.clone()),
                    UserContent::CommandExecution { stdout, .. } => Some(stdout.clone()),
                    UserContent::AppCommand { response, .. } => {
                        response.as_ref().map(|r| match r {
                            CommandResponse::Text(t) => t.clone(),
                            CommandResponse::Compact(CompactResult::Success(s)) => s.clone(),
                            _ => String::new(),
                        })
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            MessageData::Assistant { content } => content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::Text { text } => Some(text.clone()),
                    _ => None,
                })
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
                    UserContent::CommandExecution {
                        command,
                        stdout,
                        stderr,
                        exit_code,
                    } => {
                        let mut output = format!("$ {command}\n{stdout}");
                        if *exit_code != 0 {
                            output.push_str(&format!("\nExit code: {exit_code}"));
                            if !stderr.is_empty() {
                                output.push_str(&format!("\nError: {stderr}"));
                            }
                        }
                        output
                    }
                    UserContent::AppCommand { command, response } => {
                        if let Some(resp) = response {
                            let text = match resp {
                                CommandResponse::Text(msg) => msg.clone(),
                                CommandResponse::Compact(result) => match result {
                                    CompactResult::Success(summary) => summary.clone(),
                                    CompactResult::Cancelled => {
                                        "Compact command cancelled.".to_string()
                                    }
                                    CompactResult::InsufficientMessages => {
                                        "Not enough messages to compact (minimum 10 required)."
                                            .to_string()
                                    }
                                },
                            };
                            format!("/{}\n{}", command.as_command_str(), text)
                        } else {
                            format!("/{}", command.as_command_str())
                        }
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            MessageData::Assistant { content } => content
                .iter()
                .map(|c| match c {
                    AssistantContent::Text { text } => text.clone(),
                    AssistantContent::ToolCall { tool_call } => {
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

const SUMMARY_PROMPT: &str = r#"Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions.
This summary should be thorough in capturing technical details, code patterns, and architectural decisions that would be essential for continuing development work without losing context.

Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Chronologically analyze each message and section of the conversation. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like file names, full code snippets, function signatures, file edits, etc
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly.

Your summary should include the following sections:

1. Primary Request and Intent: Capture all of the user's explicit requests and intents in detail
2. Key Technical Concepts: List all important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Pay special attention to the most recent messages and include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
5. Pending Tasks: Outline any pending tasks that you have explicitly been asked to work on.
6. Current Work: Describe in detail precisely what was being worked on immediately before this summary request, paying special attention to the most recent messages from both user and assistant. Include file names and code snippets where applicable.
7. Optional Next Step: List the next step that you will take that is related to the most recent work you were doing. IMPORTANT: ensure that this step is DIRECTLY in line with the user's explicit requests, and the task you were working on immediately before this summary request. If your last task was concluded, then only list next steps if they are explicitly in line with the users request. Do not start on tangential requests without confirming with the user first.
                       If there is a next step, include direct quotes from the most recent conversation showing exactly what task you were working on and where you left off. This should be verbatim to ensure there's no drift in task interpretation.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]
   - [...]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Summary of the changes made to this file, if any]
      - [Important Code Snippet]
   - [File Name 2]
      - [Important Code Snippet]
   - [...]

4. Problem Solving:
   [Description of solved problems and ongoing troubleshooting]

5. Pending Tasks:
   - [Task 1]
   - [Task 2]
   - [...]

6. Current Work:
   [Precise description of current work]

7. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>

Please provide your summary based on the conversation so far, following this structure and ensuring precision and thoroughness in your response.

There may be additional summarization instructions provided in the included context. If so, remember to follow these instructions when creating the above summary. Examples of instructions include:
<example>
## Compact Instructions
When summarizing the conversation focus on typescript code changes and also remember the mistakes you made and how you fixed them.
</example>

<example>
# Summary instructions
When you are using compact - please focus on test output and code changes. Include file reads verbatim.
</example>"#;

/// A conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
    pub working_directory: PathBuf,
    /// The ID of the currently active message (head of the selected branch).
    /// None means use last message semantics for backward compatibility.
    pub active_message_id: Option<String>,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            working_directory: PathBuf::new(),
            active_message_id: None,
        }
    }

    pub fn add_message(&mut self, message: Message) -> bool {
        let message_id = message.id().to_string();
        let parent_id = message.parent_message_id();

        // Update active_message_id:
        // 1. If parent_id is None, this is a root message (start of new conversation)
        // 2. If parent_id matches current active_message_id (or active is None and parent is last message),
        //    we're continuing the current branch
        let should_update_active = if let Some(ref active_id) = self.active_message_id {
            // We're continuing the current branch if parent matches active
            parent_id == Some(active_id.as_str())
        } else {
            // No active branch set - use last message semantics
            parent_id.is_none() || parent_id == self.messages.last().map(|m| m.id())
        };

        let changed = if should_update_active || parent_id.is_none() {
            self.active_message_id = Some(message_id);
            true
        } else {
            false
        };

        self.messages.push(message);
        changed
    }

    pub fn clear(&mut self) -> bool {
        debug!(target:"conversation::clear", "Clearing conversation");
        self.messages.clear();
        let changed = self.active_message_id.is_some();
        self.active_message_id = None;
        changed
    }

    pub fn add_tool_result(&mut self, tool_use_id: String, message_id: String, result: ToolResult) {
        let parent_id = self.messages.last().map(|m| m.id().to_string());
        self.add_message(Message {
            data: MessageData::Tool {
                tool_use_id,
                result,
            },
            timestamp: Message::current_timestamp(),
            id: message_id,
            parent_message_id: parent_id,
        });
    }

    /// Find the tool name by its ID by searching through assistant messages with tool calls
    pub fn find_tool_name_by_id(&self, tool_id: &str) -> Option<String> {
        for message in self.messages.iter() {
            if let MessageData::Assistant { content, .. } = &message.data {
                for content_block in content {
                    if let AssistantContent::ToolCall { tool_call } = content_block {
                        if tool_call.id == tool_id {
                            return Some(tool_call.name.clone());
                        }
                    }
                }
            }
        }
        None
    }

    /// Compact the conversation by summarizing older messages in the active thread
    pub async fn compact(
        &mut self,
        api_client: &ApiClient,
        model: Model,
        token: CancellationToken,
    ) -> crate::error::Result<CompactResult> {
        // Get only the active thread
        let thread = self.get_active_thread();

        // Skip if we don't have enough messages to compact
        if thread.len() < 10 {
            return Ok(CompactResult::InsufficientMessages);
        }

        // Build prompt from active thread only
        let mut prompt_messages: Vec<Message> = thread.into_iter().cloned().collect();
        let last_msg_id = prompt_messages.last().map(|m| m.id().to_string());

        prompt_messages.push(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: SUMMARY_PROMPT.to_string(),
                }],
            },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            parent_message_id: last_msg_id.clone(),
        });

        let summary = tokio::select! {
            biased;
            result = api_client.complete(
                model,
                prompt_messages,
                None,
                None,
                token.clone(),
            ) => result.map_err(crate::error::Error::Api)?,
            _ = token.cancelled() => {
                return Ok(CompactResult::Cancelled);
            }
        };

        let summary_text = summary.extract_text();

        // Create a summary marker message (DO NOT clear messages)
        let timestamp = Message::current_timestamp();
        let summary_id = Message::generate_id("user", timestamp);

        // Add the summary as a user message continuing the active thread
        let summary_message = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: format!("[COMPACTED SUMMARY]\n\n{summary_text}"),
                }],
            },
            timestamp,
            id: summary_id.clone(),
            parent_message_id: last_msg_id, // Continue from the last message in the thread
        };

        self.messages.push(summary_message);

        // Update active_message_id to the summary marker
        self.active_message_id = Some(summary_id);

        Ok(CompactResult::Success(summary_text))
    }

    /// Edit a message non-destructively by creating a new branch.
    /// Returns the ID of the new message if successful.
    pub fn edit_message(
        &mut self,
        message_id: &str,
        new_content: Vec<UserContent>,
    ) -> Option<String> {
        // Find the message to edit
        let message_to_edit = self.messages.iter().find(|m| m.id() == message_id)?;

        // Only allow editing user messages for now
        if !matches!(&message_to_edit.data, MessageData::User { .. }) {
            return None;
        }

        // Get the parent_message_id from the original message
        let parent_id = message_to_edit.parent_message_id().map(|s| s.to_string());

        // Create the new message as a branch from the same parent
        let new_message_id = Message::generate_id("user", Message::current_timestamp());
        let edited_message = Message {
            data: MessageData::User {
                content: new_content,
            },
            timestamp: Message::current_timestamp(),
            id: new_message_id.clone(),
            parent_message_id: parent_id,
        };

        // Add the edited message (original remains in history)
        self.messages.push(edited_message);

        // Update active_message_id to the new branch head
        self.active_message_id = Some(new_message_id.clone());

        Some(new_message_id)
    }

    /// Switch to another branch by setting active_message_id
    pub fn checkout(&mut self, message_id: &str) -> bool {
        // Verify the message exists
        if self.messages.iter().any(|m| m.id() == message_id) {
            self.active_message_id = Some(message_id.to_string());
            true
        } else {
            false
        }
    }

    /// Get messages in the currently active thread
    pub fn get_active_thread(&self) -> Vec<&Message> {
        if self.messages.is_empty() {
            return Vec::new();
        }

        // Determine the head of the active thread
        let head_id = if let Some(ref active_id) = self.active_message_id {
            // Use the explicitly set active message
            active_id.as_str()
        } else {
            // Backward compatibility: use last message
            self.messages.last().map(|m| m.id()).unwrap_or("")
        };

        // Find the head message
        let mut current_msg = self.messages.iter().find(|m| m.id() == head_id);
        if current_msg.is_none() {
            // If active_message_id is invalid, fall back to last message
            current_msg = self.messages.last();
        }

        let mut result = Vec::new();
        let id_map: HashMap<&str, &Message> = self.messages.iter().map(|m| (m.id(), m)).collect();

        // Walk backwards from head to root
        while let Some(msg) = current_msg {
            result.push(msg);

            // Find parent message using the id_map
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

    /// Get messages in the current active branch by following parent links from the last message
    /// This is a thin wrapper around get_active_thread for backward compatibility
    pub fn get_thread_messages(&self) -> Vec<&Message> {
        self.get_active_thread()
    }
}

#[cfg(test)]
mod tests {
    use crate::app::conversation::{
        AssistantContent, Conversation, Message, MessageData, UserContent,
    };

    /// Helper function to create a user message for testing
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

    /// Helper function to create an assistant message for testing
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
        let mut conversation = Conversation::new();

        // 1. Build an initial conversation
        let msg1 = create_user_message("msg1", None, "What is Rust?");
        conversation.add_message(msg1.clone());

        let msg2 =
            create_assistant_message("msg2", Some("msg1"), "A systems programming language.");
        conversation.add_message(msg2.clone());

        let msg3 = create_user_message("msg3", Some("msg2"), "Is it fast?");
        conversation.add_message(msg3.clone());

        let msg4 = create_assistant_message("msg4", Some("msg3"), "Yes, it is very fast.");
        conversation.add_message(msg4.clone());

        // 2. Edit the *first* user message
        let edited_id = conversation
            .edit_message(
                "msg1",
                vec![UserContent::Text {
                    text: "What is Golang?".to_string(),
                }],
            )
            .unwrap();

        // 3. Check the state after editing
        let messages_after_edit = conversation.get_thread_messages();
        let message_ids_after_edit: Vec<&str> =
            messages_after_edit.iter().map(|m| m.id()).collect();

        assert_eq!(
            message_ids_after_edit.len(),
            1,
            "Active thread should only show the edited message"
        );
        assert_eq!(message_ids_after_edit[0], edited_id.as_str());

        // Verify original branch still exists in messages
        assert!(conversation.messages.iter().any(|m| m.id() == "msg1"));
        assert!(conversation.messages.iter().any(|m| m.id() == "msg2"));
        assert!(conversation.messages.iter().any(|m| m.id() == "msg3"));
        assert!(conversation.messages.iter().any(|m| m.id() == "msg4"));

        // 4. Add a new message to the new branch of conversation
        let msg5 = create_assistant_message(
            "msg5",
            Some(&edited_id),
            "A systems programming language from Google.",
        );
        conversation.add_message(msg5.clone());

        // 5. Check the final state of the conversation
        let final_messages = conversation.get_thread_messages();
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
        let mut conversation = Conversation::new();

        // 1. Initial conversation
        let msg1 = create_user_message("msg1", None, "hello");
        conversation.add_message(msg1.clone());

        let msg2 = create_assistant_message("msg2", Some("msg1"), "world");
        conversation.add_message(msg2.clone());

        // This is the message that will be "edited out"
        let msg3_original = create_user_message("msg3_original", Some("msg2"), "thanks");
        conversation.add_message(msg3_original.clone());

        // 2. Edit the last user message ("thanks")
        let edited_id = conversation
            .edit_message(
                "msg3_original",
                vec![UserContent::Text {
                    text: "how are you".to_string(),
                }],
            )
            .unwrap();

        // 3. Add a new assistant message to the new branch
        let msg4 = create_assistant_message("msg4", Some(&edited_id), "I am fine");
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
            thread_message_ids.contains(&edited_id.as_str()),
            "Should contain the edited message"
        );
        assert!(thread_message_ids.contains(&"msg4"), "Should contain msg4");

        // Verify the original message still exists in the full message list
        assert!(
            conversation
                .messages
                .iter()
                .any(|m| m.id() == "msg3_original"),
            "Original message should still exist in conversation history"
        );
    }

    #[test]
    fn test_get_thread_messages_filters_other_branches() {
        let mut conversation = Conversation::new();

        // 1. Initial conversation: "hi"
        let msg1 = create_user_message("msg1", None, "hi");
        conversation.add_message(msg1.clone());

        let msg2 = create_assistant_message("msg2", Some("msg1"), "Hello! How can I help?");
        conversation.add_message(msg2.clone());

        // 2. User says "thanks" (this will be edited out)
        let msg3_original = create_user_message("msg3_original", Some("msg2"), "thanks");
        conversation.add_message(msg3_original.clone());

        let msg4_original =
            create_assistant_message("msg4_original", Some("msg3_original"), "You're welcome!");
        conversation.add_message(msg4_original.clone());

        // 3. Edit the "thanks" message to "how are you"
        let edited_id = conversation
            .edit_message(
                "msg3_original",
                vec![UserContent::Text {
                    text: "how are you".to_string(),
                }],
            )
            .unwrap();

        // 4. Add assistant response in the new branch
        let msg4_new = create_assistant_message(
            "msg4_new",
            Some(&edited_id),
            "I'm doing well, thanks for asking! Ready to help with any software engineering tasks you have.",
        );
        conversation.add_message(msg4_new.clone());

        // 5. User asks "what messages have I sent you?"
        let msg5 = create_user_message("msg5", Some("msg4_new"), "what messages have I sent you?");
        conversation.add_message(msg5.clone());

        // 6. Get messages for the current thread - this should NOT include "thanks"
        let thread_messages = conversation.get_thread_messages();

        // Extract the user messages
        let user_messages: Vec<String> = thread_messages
            .iter()
            .filter(|m| matches!(m.data, MessageData::User { .. }))
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

        // CRITICAL: Should NOT contain "thanks" from the other branch
        assert!(
            !user_messages.contains(&"thanks".to_string()),
            "Should NOT contain 'thanks' from the non-active branch"
        );

        // But the original message should still exist in the full conversation
        assert!(
            conversation
                .messages
                .iter()
                .any(|m| m.id() == "msg3_original"),
            "Original 'thanks' message should still exist in conversation history"
        );
    }

    #[test]
    fn test_checkout_branch() {
        let mut conversation = Conversation::new();

        // Create initial conversation
        let msg1 = create_user_message("msg1", None, "hello");
        conversation.add_message(msg1.clone());

        let msg2 = create_assistant_message("msg2", Some("msg1"), "hi there");
        conversation.add_message(msg2.clone());

        // Edit to create a branch
        let edited_id = conversation
            .edit_message(
                "msg1",
                vec![UserContent::Text {
                    text: "goodbye".to_string(),
                }],
            )
            .unwrap();

        // Verify we're on the new branch
        assert_eq!(conversation.active_message_id, Some(edited_id.clone()));
        let thread = conversation.get_active_thread();
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].id(), edited_id);

        // Checkout the original branch
        assert!(conversation.checkout("msg2"));
        assert_eq!(conversation.active_message_id, Some("msg2".to_string()));

        // Verify we're back on the original branch
        let thread = conversation.get_active_thread();
        assert_eq!(thread.len(), 2);
        assert_eq!(thread[0].id(), "msg1");
        assert_eq!(thread[1].id(), "msg2");

        // Try to checkout non-existent message
        assert!(!conversation.checkout("non-existent"));
        assert_eq!(conversation.active_message_id, Some("msg2".to_string()));
    }

    #[test]
    fn test_active_message_id_tracking() {
        let mut conversation = Conversation::new();

        // Initially no active message
        assert_eq!(conversation.active_message_id, None);

        // Add root message - should become active
        let msg1 = create_user_message("msg1", None, "hello");
        conversation.add_message(msg1);
        assert_eq!(conversation.active_message_id, Some("msg1".to_string()));

        // Add response - should update active
        let msg2 = create_assistant_message("msg2", Some("msg1"), "hi");
        conversation.add_message(msg2);
        assert_eq!(conversation.active_message_id, Some("msg2".to_string()));

        // Add another branch from msg1
        let msg3 = create_user_message("msg3", Some("msg1"), "different question");
        conversation.add_message(msg3);
        // Should NOT update active since we're not continuing from current active
        assert_eq!(conversation.active_message_id, Some("msg2".to_string()));

        // Continue from active
        let msg4 = create_user_message("msg4", Some("msg2"), "follow up");
        conversation.add_message(msg4);
        assert_eq!(conversation.active_message_id, Some("msg4".to_string()));
    }

    #[tokio::test]
    async fn test_thread_aware_compaction() {
        // This test would require mocking the API client
        // For now, we'll test the logic without actually calling the API

        let mut conversation = Conversation::new();

        // Create a conversation with branches
        for i in 0..12 {
            let parent_id = if i == 0 {
                None
            } else {
                Some(format!("msg{}", i - 1))
            };
            let msg = create_user_message(
                &format!("msg{i}"),
                parent_id.as_deref(),
                &format!("message {i}"),
            );
            conversation.add_message(msg);
        }

        // Create a branch from message 5
        let branch_msg = create_user_message("branch1", Some("msg5"), "branch message");
        conversation.add_message(branch_msg);

        // The active thread should still be the main branch
        let thread = conversation.get_active_thread();
        assert_eq!(thread.len(), 12);

        // After compaction (mocked), original messages should still exist
        // and active_message_id should point to the summary
        let original_count = conversation.messages.len();
        assert_eq!(original_count, 13); // 12 main + 1 branch
    }
}
