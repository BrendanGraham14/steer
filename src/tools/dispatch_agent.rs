use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::api::Client as ApiClient;
use crate::api::messages::MessageContent;
use crate::api::messages::StructuredContent;
// Import necessary types for tool use
use crate::api::CompletionResponse;
// Use qualified paths to distinguish between the ContentBlock types
use crate::api::messages::{
    ContentBlock as MessageContentBlock, Message, convert_api_content_to_message_content,
};
use crate::api::tools::ToolResult;
use crate::tools::ToolError;
// Add CancellationToken import
use tokio_util::sync::CancellationToken;
// Add schemars import
use crate::api::Model;
use crate::config::LlmConfig;
use coder_macros::tool; // Import tool modules

/// Dispatch Agent implementation
pub struct DispatchAgent {
    // Store the API client instead of just the key
    api_client: ApiClient,
    // Tool executor for read-only tools
    tool_executor: crate::app::ToolExecutor,
}

// Derive JsonSchema for parameters
#[derive(Deserialize, Debug, JsonSchema)]
struct DispatchAgentParams {
    /// The task for the agent to perform
    prompt: String,
}

tool! {
    DispatchAgentTool {
        params: DispatchAgentParams,
        description: r#"Launch a new agent that has access to the following tools: glob, grep, ls, view. When you are searching for a keyword or file and are not confident that you will find the right match on the first try, use the Agent tool to perform the search for you. For example:
- If you are searching for a keyword like "config" or "logger", the Agent tool is appropriate
- If you want to read a specific file path, use the View or GlobTool tool instead of the Agent tool, to find the match more quickly
- If you are searching for a specific class definition like "class Foo", use the GlobTool tool instead, to find the match more quickly

Usage notes:
1. Launch multiple agents concurrently whenever possible, to maximize performance; to do that, use a single message with multiple tool uses
2. When the agent is done, it will return a single message back to you. The result returned by the agent is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result.
3. Each agent invocation is stateless. You will not be able to send additional messages to the agent, nor will the agent be able to communicate with you outside of its final report. Therefore, your prompt should contain a highly detailed task description for the agent to perform autonomously and you should specify exactly what information the agent should return back to you in its final and only message to you.
4. The agent's outputs should generally be trusted
5. IMPORTANT: The agent can not modify files. If you want to modify files, do it directly instead of going through the agent."#,
        name: "dispatch_agent",
        require_approval: false
    }

    async fn run(
        _tool: &DispatchAgentTool,
        params: DispatchAgentParams,
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError> {
        let token = token.unwrap_or_default();
        let agent = DispatchAgent::new()
            .map_err(|e| ToolError::execution("DispatchAgent", e))?;
        agent
            .execute(&params.prompt, token)
            .await
            .map_err(|e| ToolError::execution("dispatch_agent", e))
    }
}

impl DispatchAgent {
    pub fn new() -> Result<Self> {
        // Use Client::new and LlmConfig::from_env
        let llm_config = LlmConfig::from_env()?;
        let api_client = ApiClient::new(&llm_config);
        let tool_executor = crate::app::ToolExecutor::read_only();

        Ok(Self {
            api_client,
            tool_executor,
        })
    }

    pub fn with_api_client(api_client: ApiClient) -> Self {
        // Allow injecting an existing client (e.g., from App)
        let tool_executor = crate::app::ToolExecutor::read_only();
        Self {
            api_client,
            tool_executor,
        }
    }

    /// Execute the dispatch agent with a prompt
    pub async fn execute(&self, prompt: &str, token: CancellationToken) -> Result<String> {
        // Use the tool executor to get API tools
        let available_tools = self.tool_executor.to_api_tools();
        let system_prompt = self.create_system_prompt()?;

        // Initial message list using the correct Message type
        let mut messages: Vec<Message> = vec![Message {
            id: None,
            role: "user".to_string(),
            content: MessageContent::Text {
                content: prompt.to_string(),
            },
        }];

        const MAX_ITERATIONS: usize = 5;

        for _ in 0..MAX_ITERATIONS {
            if token.is_cancelled() {
                return Err(anyhow::anyhow!("DispatchAgent execution cancelled"));
            }

            // Call the API using the stored api_client
            let completion: CompletionResponse = self
                .api_client
                .complete(
                    Model::Claude3_7Sonnet20250219,
                    messages.clone(), // Clone messages for each API call
                    Some(system_prompt.clone()),
                    Some(available_tools.clone()),
                    token.clone(), // Pass the token
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
                    role: "assistant".to_string(),
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

                    // Execute the tool using our tool executor
                    let result = self
                        .tool_executor
                        .execute_tool_with_cancellation(&tool_call, token.clone())
                        .await;

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

                messages.push(Message {
                    role: "user".to_string(),
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
