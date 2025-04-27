use anyhow::Result;
use std::env;

use crate::api::messages::MessageContent;
use crate::api::messages::StructuredContent;
// Import the API client
use crate::api::Client as ApiClient;
// Import necessary types for tool use
use crate::api::CompletionResponse;
// Use qualified paths to distinguish between the ContentBlock types
use crate::api::messages::{
    ContentBlock as MessageContentBlock, Message, convert_api_content_to_message_content,
};
use crate::api::tools::{Tool, ToolResult};
use crate::app::Role; // Use Role from app module as it's likely the one used elsewhere
use crate::tools;

/// Dispatch Agent implementation
pub struct DispatchAgent {
    // Store the API client instead of just the key
    api_client: ApiClient,
}

impl DispatchAgent {
    pub fn new() -> Self {
        // Default implementation gets the API key from environment
        let api_key = env::var("CLAUDE_API_KEY").unwrap_or_else(|_| String::from(""));

        // Create the API client
        let api_client = ApiClient::new(&api_key).with_model("claude-3-haiku-20240307");

        Self { api_client }
    }

    pub fn with_api_key(api_key: String) -> Self {
        // Create the API client with the provided key
        let api_client = ApiClient::new(&api_key).with_model("claude-3-haiku-20240307");
        Self { api_client }
    }

    /// Execute the dispatch agent with a prompt
    pub async fn execute(&self, prompt: &str) -> Result<String> {
        let available_tools = Tool::read_only();
        let system_prompt = self.create_system_prompt()?;

        // Initial message list using the correct Message type
        let mut messages: Vec<Message> = vec![Message {
            id: None,
            role: Role::User.to_string(),
            content: MessageContent::Text {
                content: prompt.to_string(),
            },
        }];

        // Max loop iterations to prevent infinite loops
        const MAX_ITERATIONS: usize = 5;

        for _ in 0..MAX_ITERATIONS {
            // Call the API using the stored api_client
            let completion: CompletionResponse = self
                .api_client
                .complete(
                    messages.clone(), // Clone messages for each API call
                    Some(system_prompt.clone()),
                    Some(available_tools.clone()),
                )
                .await?;

            // IMPORTANT: Extract tool calls BEFORE moving completion.content
            let tool_calls = completion.extract_tool_calls();

            // ALSO: Extract text content before moving completion.content
            let response_text = completion.extract_text();

            // Convert api::ContentBlock to messages::ContentBlock using the extracted function
            let message_content_blocks = convert_api_content_to_message_content(completion.content);

            // Add the assistant's response to the message history
            // Only add if there are content blocks (avoids empty assistant messages if conversion filters everything)
            if !message_content_blocks.is_empty() {
                messages.push(Message {
                    id: None, // Assuming API response doesn't give us a message ID directly here
                    role: Role::Assistant.to_string(),
                    content: MessageContent::StructuredContent {
                        content: StructuredContent(message_content_blocks),
                    },
                });
            }

            // Check for tool calls (using the variable extracted earlier)
            if tool_calls.is_empty() {
                // No tool calls, return the final text response
                // Use the text extracted earlier
                return Ok(response_text);
            } else {
                // Execute tool calls and collect results
                let mut tool_results: Vec<ToolResult> = Vec::new();
                for tool_call in tool_calls {
                    crate::utils::logging::debug(
                        "DispatchAgent.execute",
                        &format!("Dispatch agent executing tool: {}", tool_call.name),
                    );

                    let result = tools::execute_tool(&tool_call.name, &tool_call.parameters).await;

                    let output = match result {
                        Ok(output) => output,
                        Err(e) => format!("Error executing tool {}: {}", tool_call.name, e),
                    };

                    tool_results.push(ToolResult {
                        tool_call_id: tool_call.id,
                        output,
                    });
                }

                // Convert ToolResult (from tools module) to messages::ContentBlock::ToolResult
                let result_blocks: Vec<MessageContentBlock> = tool_results
                    .into_iter()
                    .map(|tool_result| {
                        let is_error = tool_result.output.starts_with("Error:");
                        MessageContentBlock::ToolResult {
                            tool_use_id: tool_result.tool_call_id,
                            // Wrap the string output in a Text block as required by the schema
                            content: vec![MessageContentBlock::Text {
                                text: tool_result.output,
                            }],
                            is_error: if is_error { Some(true) } else { None },
                        }
                    })
                    .collect();

                // Add tool results as a User message with structured content
                messages.push(Message {
                    role: Role::User.to_string(),
                    content: MessageContent::StructuredContent {
                        content: StructuredContent(result_blocks),
                    },
                    id: None, // Or generate a unique ID if needed
                });
                // Continue the loop to send results back to the model
            }
        }

        // If the loop completes without returning, it means the max iterations were hit.
        Err(anyhow::anyhow!(
            "DispatchAgent reached maximum iterations without a final response."
        ))
    }

    /// Create the system prompt for the dispatch agent
    fn create_system_prompt(&self) -> Result<String> {
        // Get the environment information
        let env_info = crate::app::EnvironmentInfo::collect()?;

        // Read the dispatch agent prompt template
        let dispatch_prompt = r#"You are an agent for a CLI-based coding tool. Given the user's prompt, you should use the tools available to you to answer the user's question.

Notes:
1. IMPORTANT: You should be concise, direct, and to the point, since your responses will be displayed on a command line interface. Answer the user's question directly, without elaboration, explanation, or details. One word answers are best. Avoid introductions, conclusions, and explanations. You MUST avoid text before/after your response, such as "The answer is <answer>.", "Here is the content of the file..." or "Based on the information provided, the answer is..." or "Here is what I will do next...".
2. When relevant, share file names and code snippets relevant to the query
3. Any file paths you return in your final response MUST be absolute. DO NOT use relative paths."#;

        // Create a formatted environment info section
        let env_info_str = format!(
            "Here is useful information about the environment you are running in:\n\
            <env>\n\
            Working directory: {}\n\
            Is directory a git repo: {}\n\
            Platform: {}\n\
            Today's date: {}\n\
            </env>",
            env_info.working_directory.display(),
            if env_info.is_git_repo { "Yes" } else { "No" },
            env_info.platform,
            env_info.date
        );

        // Combine the prompt template with the environment info
        // Tool info is handled by the API client
        let full_prompt = format!("{}\n\n{}", dispatch_prompt, env_info_str);

        Ok(full_prompt)
    }
}
