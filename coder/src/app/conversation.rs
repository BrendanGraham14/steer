use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tools::ToolCall;

use crate::api::Client as ApiClient;
use crate::api::Model;

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
    /// Cancel current operation
    Cancel,
    /// Show help information
    Help,
    /// Unknown/unrecognized command
    Unknown { command: String },
}

impl AppCommandType {
    /// Get the command string representation
    pub fn as_command_str(&self) -> String {
        match self {
            AppCommandType::Model { target } => {
                if let Some(model) = target {
                    format!("model {}", model)
                } else {
                    "model".to_string()
                }
            }
            AppCommandType::Clear => "clear".to_string(),
            AppCommandType::Compact => "compact".to_string(),
            AppCommandType::Cancel => "cancel".to_string(),
            AppCommandType::Help => "help".to_string(),
            AppCommandType::Unknown { command } => command.clone(),
        }
    }

    /// Parse a command string into an AppCommandType
    pub fn from_command_str(command: &str) -> Self {
        let parts: Vec<&str> = command.trim_start_matches('/').split_whitespace().collect();
        match parts.first().map(|s| *s) {
            Some("model") => AppCommandType::Model {
                target: parts.get(1).map(|s| s.to_string()),
            },
            Some("clear") => AppCommandType::Clear,
            Some("compact") => AppCommandType::Compact,
            Some("cancel") => AppCommandType::Cancel,
            Some("help") => AppCommandType::Help,
            _ => AppCommandType::Unknown {
                command: command.to_string(),
            },
        }
    }
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
    <command>{}</command>
    <stdout>{}</stdout>
    <stderr>{}</stderr>
    <exit_code>{}</exit_code>
</executed_command>"#,
            command, stdout, stderr, exit_code
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

/// Result from a tool execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResult {
    Success { output: String },
    Error { error: String },
}

/// A message in the conversation, with role-specific content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    User {
        content: Vec<UserContent>,
        timestamp: u64,
        id: String,
    },
    Assistant {
        content: Vec<AssistantContent>,
        timestamp: u64,
        id: String,
    },
    Tool {
        tool_use_id: String,
        result: ToolResult,
        timestamp: u64,
        id: String,
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

    /// Helper to get current timestamp
    pub fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
    }

    /// Helper to generate unique IDs
    pub fn generate_id(prefix: &str, timestamp: u64) -> String {
        let random_suffix = format!("{:04x}", (timestamp % 10000));
        format!("{}_{}{}", prefix, timestamp, random_suffix)
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
                        let mut output = format!("$ {}\n{}", command, stdout);
                        if *exit_code != 0 {
                            output.push_str(&format!("\nExit code: {}", exit_code));
                            if !stderr.is_empty() {
                                output.push_str(&format!("\nError: {}", stderr));
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
                                    CompactResult::Cancelled => "Compact command cancelled.".to_string(),
                                    CompactResult::InsufficientMessages => "Not enough messages to compact (minimum 10 required).".to_string(),
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
            Message::Tool {
                tool_use_id,
                result,
                ..
            } => match result {
                ToolResult::Success { output } => {
                    format!("[Tool Result for {}: {}]", tool_use_id, output)
                }
                ToolResult::Error { error } => {
                    format!("[Tool Error for {}: {}]", tool_use_id, error)
                }
            },
        }
    }

    /// Extract all text content from the message
    pub fn extract_text(&self) -> String {
        match self {
            Message::User { content, .. } => content
                .iter()
                .filter_map(|c| match c {
                    UserContent::Text { text } => Some(text.clone()),
                    UserContent::CommandExecution { stdout, .. } => Some(stdout.clone()),
                    UserContent::AppCommand { response, .. } => response.as_ref().map(|r| match r {
                        CommandResponse::Text(msg) => msg.clone(),
                        CommandResponse::Compact(result) => match result {
                            CompactResult::Success(summary) => summary.clone(),
                            CompactResult::Cancelled => "Compact command cancelled.".to_string(),
                            CompactResult::InsufficientMessages => "Not enough messages to compact (minimum 10 required).".to_string(),
                        },
                    }),
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
            Message::Tool { result, .. } => match result {
                ToolResult::Success { output } => output.clone(),
                ToolResult::Error { error } => format!("Error: {}", error),
            },
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
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn add_tool_result(&mut self, tool_use_id: String, result: String) {
        self.add_message(Message::Tool {
            tool_use_id,
            result: ToolResult::Success { output: result },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("tool", Message::current_timestamp()),
        });
    }

    pub fn add_tool_error(&mut self, tool_use_id: String, error: String) {
        self.add_message(Message::Tool {
            tool_use_id,
            result: ToolResult::Error { error },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("tool", Message::current_timestamp()),
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
        token: CancellationToken,
    ) -> anyhow::Result<CompactResult> {
        // Skip if we don't have enough messages to compact
        if self.messages.len() < 10 {
            return Ok(CompactResult::InsufficientMessages);
        }
        let mut prompt_messages = self.messages.clone();
        prompt_messages.push(Message::User {
            content: vec![UserContent::Text {
                text: SUMMARY_PROMPT.to_string(),
            }],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
        });

        let summary = tokio::select! {
            biased;
            result = api_client.complete(
                Model::Claude3_7Sonnet20250219,
                prompt_messages,
                None,
                None,
                token.clone(),
            ) => result?,
            _ = token.cancelled() => {
                return Ok(CompactResult::Cancelled);
            }
        };
        
        let summary_text = summary.extract_text();

        // Clear messages and add compaction marker
        self.messages.clear();
        let timestamp = Message::current_timestamp();
        
        // Add the summary as a user message with a clear marker
        self.add_message(Message::User {
            content: vec![UserContent::Text {
                text: format!("[CONVERSATION COMPACTED]\n\nPrevious conversation summary:\n{}", summary_text),
            }],
            timestamp,
            id: Message::generate_id("user", timestamp),
        });

        Ok(CompactResult::Success(summary_text))
    }
}
