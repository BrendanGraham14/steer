use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::{
    Client as ApiClient, Model,
    messages::{
        Message as ApiMessage, MessageContent as ApiMessageContent, MessageRole as ApiMessageRole,
    },
    tools::{Tool as ApiTool, ToolCall as ApiToolCall},
};

use crate::app::{AgentEvent, AgentExecutor, AgentExecutorRunRequest};
use crate::config::LlmConfig;
use crate::tools::ToolError;
use coder_macros::tool;
use tokio_util::sync::CancellationToken;

#[derive(Deserialize, Debug, JsonSchema, Serialize)]
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

        // --- Setup AgentExecutor dependencies ---
        let llm_config = LlmConfig::from_env()
             .map_err(|e| ToolError::execution(DISPATCH_AGENT_TOOL_NAME, anyhow::anyhow!("Failed to load LLM config: {}", e)))?;
        let api_client = Arc::new(ApiClient::new(&llm_config)); // Create ApiClient and wrap in Arc
        let agent_executor = AgentExecutor::new(api_client);

        // Create a read-only ToolExecutor specifically for this agent run
        let read_only_tool_executor = Arc::new(crate::app::ToolExecutor::read_only());

        // Get available tools before moving read_only_tool_executor into the closure
        let available_tools: Vec<ApiTool> = read_only_tool_executor.to_api_tools();

        // Define the tool executor callback for the agent
        let tool_executor_callback =
            move |tool_call: ApiToolCall, callback_token: CancellationToken| {
                let executor = read_only_tool_executor.clone();
                async move {
                    executor
                        .execute_tool_with_cancellation(&tool_call, callback_token)
                        .await
                }
            };

        // --- Prepare for AgentExecutor ---
        let initial_messages = vec![ApiMessage {
            id: None,
            role: ApiMessageRole::User,
            content: ApiMessageContent::Text { content: params.prompt },
        }];

        let system_prompt = create_dispatch_agent_system_prompt()
            .map_err(|e| ToolError::execution(DISPATCH_AGENT_TOOL_NAME, anyhow::anyhow!("Failed to create system prompt: {}", e)))?;

        // Use a channel to receive events, though we might just aggregate the final result here.
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

        // --- Run AgentExecutor ---
        let operation_result = agent_executor
            .run(
                AgentExecutorRunRequest
                 {
                    model: Model::Claude3_7Sonnet20250219, // Or make configurable?
                    initial_messages,
                    system_prompt: Some(system_prompt),
                    available_tools,
                    tool_executor_callback,
                },
                event_tx,
                token,
            )
            .await;

        // --- Process Result ---
        // We need the final text response from the agent.
        // Collect text from events or the final message.
        let mut final_text = String::new();
        // let mut final_message_content: Option<ApiMessage> = None;

        // Drain remaining events
        while let Ok(event) = event_rx.try_recv() {
             match event {
                 AgentEvent::AssistantMessagePart(text) => final_text.push_str(&text),
                 AgentEvent::AssistantMessageFinal(msg) => {
                     // Extract text if we haven't gotten it from parts
                     if final_text.is_empty() {
                         final_text = msg.content.extract_text();
                     }
                    // final_message_content = Some(msg)
                 },
                 // Ignore other events for this tool's purpose
                 _ => {}
             }
        }


        match operation_result {
            Ok(message) => {
                 // If we still don't have text, extract from final message object
                 if final_text.is_empty() {
                     final_text = message.content.extract_text();
                 }
                 Ok(final_text)
            }
            Err(e) => {
                 Err(ToolError::execution(DISPATCH_AGENT_TOOL_NAME, e.into_anyhow_error()))
            }
        }
    }
}

pub fn create_dispatch_agent_system_prompt() -> Result<String> {
    let env_info = crate::app::EnvironmentInfo::collect()?;
    let dispatch_prompt = format!(
        r#"You are an agent for a CLI-based coding tool. Given the user's prompt, you should use the tools available to you to answer the user's question.

Notes:
1. IMPORTANT: You should be concise, direct, and to the point, since your responses will be displayed on a command line interface. Answer the user's question directly, without elaboration, explanation, or details. One word answers are best. Avoid introductions, conclusions, and explanations. You MUST avoid text before/after your response, such as "The answer is <answer>.", "Here is the content of the file..." or "Based on the information provided, the answer is..." or "Here is what I will do next...".
2. When relevant, share file names and code snippets relevant to the query
3. Any file paths you return in your final response MUST be absolute. DO NOT use relative paths.

{}
"#,
        env_info.as_context()
    );

    Ok(dispatch_prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotenv::dotenv;

    #[tokio::test]
    #[ignore] // Requires API key and network call
    async fn test_dispatch_agent() {
        // Load environment variables from .env file
        dotenv().ok();

        // Ensure API key is available for the test
        let _api_key =
            std::env::var("CLAUDE_API_KEY").expect("CLAUDE_API_KEY must be set for this test");

        // Setup necessary context for the tool run method
        let temp_dir = tempfile::tempdir().unwrap(); // Create a temp directory for the environment
        std::fs::write(
            temp_dir.path().join("search_code.rs"),
            "fn find_stuff() {}
fn search_database() {}
",
        )
        .unwrap();
        let token = CancellationToken::new(); // Create cancellation token

        // Test prompt that should search for specific code
        let prompt = "Find all files that contain definitions of functions or methods related to search or find operations. Return only the absolute file path.";

        let params = DispatchAgentParams {
            prompt: prompt.to_string(),
        };

        // Instantiate the tool struct (assuming default if no specific state needed)
        let tool_instance = DispatchAgentTool; // Assuming it's a unit struct or has Default impl

        // Execute the agent using the run method
        let result = run(&tool_instance, params, Some(token)).await;

        // Check if we got a valid response
        assert!(result.is_ok(), "Agent execution failed: {:?}", result.err());
        let response = result.unwrap();
        assert!(!response.is_empty(), "Response should not be empty");
        assert!(
            response.contains("search_code.rs"),
            "Response should contain the file path"
        ); // Check for expected content

        println!("Dispatch agent response: {}", response);
        println!("Dispatch agent test passed successfully!");
    }
}
