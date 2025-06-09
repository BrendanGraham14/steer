use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tools::ToolCall;
use tracing::debug;

use crate::api::Client as ApiClient;
use crate::api::Model;
use crate::api::messages::MessageRole as ApiMessageRole;
use strum_macros::Display;
use tokio_util::sync::CancellationToken;

/// Role in the conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Copy, Display)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

/// Represents a block of content within a single message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageContentBlock {
    Text(String),
    ToolCall(ToolCall),
    ToolResult { tool_use_id: String, result: String },
    // TODO: support attachments
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content_blocks: Vec<MessageContentBlock>,
    pub timestamp: u64,
    pub id: String,
}

impl TryFrom<crate::api::messages::Message> for Message {
    type Error = anyhow::Error;

    fn try_from(api_message: crate::api::messages::Message) -> Result<Self, Self::Error> {
        use crate::api::messages::{MessageContent, MessageRole};

        // Convert role
        let role = match api_message.role {
            MessageRole::User => Role::User,
            MessageRole::Assistant => Role::Assistant,
            MessageRole::Tool => Role::Tool,
        };

        // Convert content blocks
        let content_blocks = match api_message.content {
            MessageContent::Text { content } => {
                vec![MessageContentBlock::Text(content)]
            }
            MessageContent::StructuredContent { content } => {
                // Process structured content blocks
                content
                    .0
                    .into_iter()
                    .map(|block| {
                        match block {
                            crate::api::messages::ContentBlock::Text { text } => {
                                Ok(MessageContentBlock::Text(text))
                            }
                            crate::api::messages::ContentBlock::ToolUse { id, name, input } => {
                                Ok(MessageContentBlock::ToolCall(ToolCall {
                                    id,
                                    name,
                                    parameters: input,
                                }))
                            }
                            crate::api::messages::ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                // Extract text from content blocks (usually just one text block)
                                let result = content
                                    .into_iter()
                                    .filter_map(|block| {
                                        if let crate::api::messages::ContentBlock::Text { text } =
                                            block
                                        {
                                            Some(text)
                                        } else {
                                            None
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");

                                Ok(MessageContentBlock::ToolResult {
                                    tool_use_id,
                                    result,
                                })
                            }
                        }
                    })
                    .collect::<Result<Vec<_>, anyhow::Error>>()?
            }
        };

        // Use provided ID or generate a new one
        let id = api_message.id.unwrap_or_else(|| {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_secs();

            let prefix = match role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };

            let random_suffix = format!("{:04x}", (timestamp % 10000));
            format!("{}_{}{}", prefix, timestamp, random_suffix)
        });

        Ok(Self {
            role,
            content_blocks,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_secs(),
            id,
        })
    }
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
        let prefix = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
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
        self.add_message(Message::new_with_blocks(
            Role::Tool,
            vec![MessageContentBlock::ToolResult {
                tool_use_id,
                result,
            }],
        ));
    }

    /// Find the tool name by its ID by searching through assistant messages with tool calls
    pub fn find_tool_name_by_id(&self, tool_id: &str) -> Option<String> {
        for message in self.messages.iter() {
            if message.role == Role::Assistant {
                for block in &message.content_blocks {
                    if let MessageContentBlock::ToolCall(tool_call) = block {
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
    ) -> anyhow::Result<()> {
        // Skip if we don't have enough messages to compact
        if self.messages.len() < 10 {
            return Ok(());
        }
        let mut prompt_messages = crate::api::messages::convert_conversation(self);
        prompt_messages.push(crate::api::messages::Message {
            role: ApiMessageRole::User,
            content: crate::api::messages::MessageContent::Text {
                content: SUMMARY_PROMPT.to_string(),
            },
            id: None,
        });

        let summary = api_client
            .complete(
                Model::Claude3_7Sonnet20250219,
                prompt_messages,
                None,
                None,
                token.clone(),
            )
            .await?;
        let summary_text = summary.extract_text();

        self.messages.clear();
        self.add_message(Message::new_text(
            Role::User,
            format!("Previous conversation summary:\n{}", summary_text),
        ));

        Ok(())
    }
}
