use crate::api::Client as ApiClient;
use crate::api::Model;
use conductor_tools::ToolCall;
pub use conductor_tools::result::ToolResult;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;
use uuid::Uuid;

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

/// A message in the conversation, with role-specific content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    User {
        content: Vec<UserContent>,
        timestamp: u64,
        id: String,
        /// Identifies which conversation branch/thread this message belongs to
        thread_id: Uuid,
        /// Links to the previous message in this branch (None for root messages)
        parent_message_id: Option<String>,
    },
    Assistant {
        content: Vec<AssistantContent>,
        timestamp: u64,
        id: String,
        thread_id: Uuid,
        parent_message_id: Option<String>,
    },
    Tool {
        tool_use_id: String,
        result: ToolResult,
        timestamp: u64,
        id: String,
        thread_id: Uuid,
        parent_message_id: Option<String>,
    },
}

impl Message {
    pub fn role(&self) -> Role {
        match self {
            Message::User { .. } => Role::User,
            Message::Assistant { .. } => Role::Assistant,
            Message::Tool { .. } => Role::Tool,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Message::User { id, .. } => id,
            Message::Assistant { id, .. } => id,
            Message::Tool { id, .. } => id,
        }
    }

    pub fn timestamp(&self) -> u64 {
        match self {
            Message::User { timestamp, .. } => *timestamp,
            Message::Assistant { timestamp, .. } => *timestamp,
            Message::Tool { timestamp, .. } => *timestamp,
        }
    }

    pub fn thread_id(&self) -> &Uuid {
        match self {
            Message::User { thread_id, .. } => thread_id,
            Message::Assistant { thread_id, .. } => thread_id,
            Message::Tool { thread_id, .. } => thread_id,
        }
    }

    pub fn parent_message_id(&self) -> Option<&str> {
        match self {
            Message::User {
                parent_message_id, ..
            } => parent_message_id.as_deref(),
            Message::Assistant {
                parent_message_id, ..
            } => parent_message_id.as_deref(),
            Message::Tool {
                parent_message_id, ..
            } => parent_message_id.as_deref(),
        }
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
        match self {
            Message::User { content, .. } => content
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
            Message::Assistant { content, .. } => content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Message::Tool { result, .. } => result.llm_format(),
        }
    }

    /// Get a string representation of the message content
    pub fn content_string(&self) -> String {
        match self {
            Message::User { content, .. } => content
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
            Message::Assistant { content, .. } => content
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
            Message::Tool { result, .. } => {
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
    /// The currently active thread ID
    pub current_thread_id: Uuid,
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
            current_thread_id: Self::generate_thread_id(),
        }
    }

    /// Generate a new thread ID using UUID v7 (timestamp-based)
    pub fn generate_thread_id() -> Uuid {
        Uuid::now_v7()
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn clear(&mut self) {
        debug!(target:"conversation::clear", "Clearing conversation");
        self.messages.clear();
    }

    pub fn add_tool_result(&mut self, tool_use_id: String, message_id: String, result: ToolResult) {
        let parent_id = self.messages.last().map(|m| m.id().to_string());
        self.add_message(Message::Tool {
            tool_use_id,
            result,
            timestamp: Message::current_timestamp(),
            id: message_id,
            thread_id: self.current_thread_id,
            parent_message_id: parent_id,
        });
    }

    /// Find the tool name by its ID by searching through assistant messages with tool calls
    pub fn find_tool_name_by_id(&self, tool_id: &str) -> Option<String> {
        for message in self.messages.iter() {
            if let Message::Assistant { content, .. } = message {
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

    /// Compact the conversation by summarizing older messages
    pub async fn compact(
        &mut self,
        api_client: &ApiClient,
        model: Model,
        token: CancellationToken,
    ) -> crate::error::Result<CompactResult> {
        // Skip if we don't have enough messages to compact
        if self.messages.len() < 10 {
            return Ok(CompactResult::InsufficientMessages);
        }
        let mut prompt_messages = self.messages.clone();
        let last_msg_id = self.messages.last().map(|m| m.id().to_string());
        prompt_messages.push(Message::User {
            content: vec![UserContent::Text {
                text: SUMMARY_PROMPT.to_string(),
            }],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            thread_id: self.current_thread_id,
            parent_message_id: last_msg_id,
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

        // Clear messages and add compaction marker
        self.clear();
        let timestamp = Message::current_timestamp();

        // Add the summary as a user message with a clear marker
        self.add_message(Message::User {
            content: vec![UserContent::Text {
                text: format!(
                    "[CONVERSATION COMPACTED]\n\nPrevious conversation summary:\n{summary_text}"
                ),
            }],
            timestamp,
            id: Message::generate_id("user", timestamp),
            thread_id: self.current_thread_id,
            parent_message_id: None, // New root message after compaction
        });

        Ok(CompactResult::Success(summary_text))
    }

    /// Edit a message, which removes the old message and all its children,
    /// then creates a new branch.
    pub fn edit_message(
        &mut self,
        message_id: &str,
        new_content: Vec<UserContent>,
    ) -> Option<Uuid> {
        // Find the message to edit
        let message_to_edit = self.messages.iter().find(|m| m.id() == message_id)?.clone();

        // Only allow editing user messages for now
        if !matches!(message_to_edit, Message::User { .. }) {
            return None;
        }

        // Get the parent_message_id from the original message before we remove it
        let parent_id = message_to_edit.parent_message_id().map(|s| s.to_string());

        // Find all descendants of the message to be edited
        let mut to_remove = HashSet::new();
        let mut queue = VecDeque::new();

        to_remove.insert(message_id.to_string());
        queue.push_back(message_id.to_string());

        while let Some(current_id) = queue.pop_front() {
            for msg in &self.messages {
                if msg.parent_message_id() == Some(current_id.as_str()) {
                    let child_id = msg.id().to_string();
                    if to_remove.insert(child_id.clone()) {
                        queue.push_back(child_id);
                    }
                }
            }
        }

        // Remove the old message and all its descendants
        self.messages.retain(|m| !to_remove.contains(m.id()));

        // Create a new thread ID for this branch
        let new_thread_id = Self::generate_thread_id();

        // Create the edited message with a new ID and the new thread ID
        let edited_message = Message::User {
            content: new_content,
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            thread_id: new_thread_id,
            parent_message_id: parent_id,
        };

        // Add the edited message
        self.messages.push(edited_message);

        // Update current thread to the new branch
        self.current_thread_id = new_thread_id;

        Some(new_thread_id)
    }

    /// Get messages in the current thread by following parent links
    pub fn get_thread_messages(&self) -> Vec<&Message> {
        let mut result = Vec::new();
        let mut current_msg = self
            .messages
            .iter()
            .filter(|m| m.thread_id() == &self.current_thread_id)
            .max_by_key(|m| m.timestamp());

        let id_map: HashMap<&str, &Message> = self.messages.iter().map(|m| (m.id(), m)).collect();

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
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::app::conversation::{AssistantContent, Conversation, Message, UserContent};
    use uuid::Uuid;

    /// Helper function to create a user message for testing
    fn create_user_message(
        id: &str,
        parent_id: Option<&str>,
        thread_id: Uuid,
        content: &str,
    ) -> Message {
        Message::User {
            id: id.to_string(),
            parent_message_id: parent_id.map(String::from),
            thread_id,
            content: vec![UserContent::Text {
                text: content.to_string(),
            }],
            timestamp: Message::current_timestamp(),
        }
    }

    /// Helper function to create an assistant message for testing
    fn create_assistant_message(
        id: &str,
        parent_id: Option<&str>,
        thread_id: Uuid,
        content: &str,
    ) -> Message {
        Message::Assistant {
            id: id.to_string(),
            parent_message_id: parent_id.map(String::from),
            thread_id,
            content: vec![AssistantContent::Text {
                text: content.to_string(),
            }],
            timestamp: Message::current_timestamp(),
        }
    }

    #[test]
    fn test_editing_message_in_the_middle_of_conversation() {
        let mut conversation = Conversation::new();
        let initial_thread_id = conversation.current_thread_id;

        // 1. Build an initial conversation
        let msg1 = create_user_message("msg1", None, initial_thread_id, "What is Rust?");
        conversation.add_message(msg1.clone());

        let msg2 = create_assistant_message(
            "msg2",
            Some("msg1"),
            initial_thread_id,
            "A systems programming language.",
        );
        conversation.add_message(msg2.clone());

        let msg3 = create_user_message("msg3", Some("msg2"), initial_thread_id, "Is it fast?");
        conversation.add_message(msg3.clone());

        let msg4 = create_assistant_message(
            "msg4",
            Some("msg3"),
            initial_thread_id,
            "Yes, it is very fast.",
        );
        conversation.add_message(msg4.clone());

        // 2. Edit the *first* user message
        let new_thread_id = conversation
            .edit_message(
                "msg1",
                vec![UserContent::Text {
                    text: "What is Golang?".to_string(),
                }],
            )
            .unwrap();

        // 3. Check the state after editing
        let edited_msg_id = {
            let messages_after_edit = conversation.get_thread_messages();
            let message_ids_after_edit: Vec<&str> =
                messages_after_edit.iter().map(|m| m.id()).collect();

            assert_eq!(
                message_ids_after_edit.len(),
                1,
                "History should be pruned to the single edited message."
            );
            let id = message_ids_after_edit[0];
            assert_ne!(id, "msg1");
            assert!(!message_ids_after_edit.contains(&"msg1"));
            assert!(!message_ids_after_edit.contains(&"msg2"));
            assert!(!message_ids_after_edit.contains(&"msg3"));
            assert!(!message_ids_after_edit.contains(&"msg4"));
            id.to_string()
        };

        // 4. Add a new message to the new branch of conversation
        let msg5 = create_assistant_message(
            "msg5",
            Some(&edited_msg_id),
            new_thread_id,
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
        assert_eq!(final_message_ids[0], edited_msg_id);
        assert_eq!(final_message_ids[1], "msg5");
    }

    #[test]
    fn test_get_thread_messages_after_edit() {
        let mut conversation = Conversation::new();
        let initial_thread_id = conversation.current_thread_id;

        // 1. Initial conversation
        let msg1 = create_user_message("msg1", None, initial_thread_id, "hello");
        conversation.add_message(msg1.clone());

        let msg2 = create_assistant_message("msg2", Some("msg1"), initial_thread_id, "world");
        conversation.add_message(msg2.clone());

        // This is the message that will be "edited out"
        let msg3_original =
            create_user_message("msg3_original", Some("msg2"), initial_thread_id, "thanks");
        conversation.add_message(msg3_original.clone());

        // 2. Edit the last user message ("thanks")
        let new_thread_id = conversation
            .edit_message(
                "msg3_original",
                vec![UserContent::Text {
                    text: "how are you".to_string(),
                }],
            )
            .unwrap();

        let edited_msg_id = conversation.messages.last().unwrap().id().to_string();

        // 3. Add a new assistant message to the new branch
        let msg4 =
            create_assistant_message("msg4", Some(&edited_msg_id), new_thread_id, "I am fine");
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
            thread_message_ids.contains(&edited_msg_id.as_str()),
            "Should contain the edited message"
        );
        assert!(thread_message_ids.contains(&"msg4"), "Should contain msg4");

        // CRITICAL: Should NOT contain the original, edited-out message
        assert!(
            !thread_message_ids.contains(&"msg3_original"),
            "Should NOT contain the original message that was edited"
        );
    }

    #[test]
    fn test_get_thread_messages_filters_other_branches() {
        let mut conversation = Conversation::new();
        let initial_thread_id = conversation.current_thread_id;

        // 1. Initial conversation: "hi"
        let msg1 = create_user_message("msg1", None, initial_thread_id, "hi");
        conversation.add_message(msg1.clone());

        let msg2 = create_assistant_message(
            "msg2",
            Some("msg1"),
            initial_thread_id,
            "Hello! How can I help?",
        );
        conversation.add_message(msg2.clone());

        // 2. User says "thanks" (this will be edited out)
        let msg3_original =
            create_user_message("msg3_original", Some("msg2"), initial_thread_id, "thanks");
        conversation.add_message(msg3_original.clone());

        let msg4_original = create_assistant_message(
            "msg4_original",
            Some("msg3_original"),
            initial_thread_id,
            "You're welcome!",
        );
        conversation.add_message(msg4_original.clone());

        // 3. Edit the "thanks" message to "how are you"
        let new_thread_id = conversation
            .edit_message(
                "msg3_original",
                vec![UserContent::Text {
                    text: "how are you".to_string(),
                }],
            )
            .unwrap();

        let edited_msg_id = conversation.messages.last().unwrap().id().to_string();

        // 4. Add assistant response in the new thread
        let msg4_new = create_assistant_message(
            "msg4_new",
            Some(&edited_msg_id),
            new_thread_id,
            "I'm doing well, thanks for asking! Ready to help with any software engineering tasks you have.",
        );
        conversation.add_message(msg4_new.clone());

        // 5. User asks "what messages have I sent you?"
        let msg5 = create_user_message(
            "msg5",
            Some("msg4_new"),
            new_thread_id,
            "what messages have I sent you?",
        );
        conversation.add_message(msg5.clone());

        // 6. Get messages for the current thread - this should NOT include "thanks"
        let thread_messages = conversation.get_thread_messages();

        // Extract the user messages
        let user_messages: Vec<String> = thread_messages
            .iter()
            .filter(|m| matches!(m, Message::User { .. }))
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

        // CRITICAL: Should NOT contain "thanks" from the edited-out branch
        assert!(
            !user_messages.contains(&"thanks".to_string()),
            "Should NOT contain 'thanks' from the edited-out branch"
        );
    }
}
