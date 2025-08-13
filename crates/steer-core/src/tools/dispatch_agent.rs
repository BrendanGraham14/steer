use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{
    app::{
        ApprovalDecision,
        conversation::{Message, MessageData, UserContent},
    },
    config::LlmConfigProvider,
    tools::ToolExecutor,
};

use crate::app::{AgentEvent, AgentExecutor, AgentExecutorRunRequest};
use steer_macros::tool_external as tool;
use steer_tools::tools::{GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, VIEW_TOOL_NAME};
use steer_tools::{ToolCall, ToolError, ToolSchema};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize, Debug, Serialize, JsonSchema)]
pub struct DispatchAgentParams {
    /// The task for the agent to perform
    pub prompt: String,
}

const DISPATCH_AGENT_TOOLS: [&str; 4] =
    [GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, VIEW_TOOL_NAME];

fn format_dispatch_agent_tools() -> String {
    DISPATCH_AGENT_TOOLS
        .iter()
        .map(|tool| tool.to_string())
        .collect::<Vec<String>>()
        .join(", ")
}

tool! {
    pub struct DispatchAgentTool {
        pub llm_config_provider: Arc<LlmConfigProvider>,
        pub workspace: Arc<dyn crate::workspace::Workspace>,
    } {
        params: DispatchAgentParams,
        output: steer_tools::result::AgentResult,
        variant: Agent,
        description: &format!(r#"Launch a new agent that has access to the following tools: {}. When you are searching for a keyword or file and are not confident that you will find the right match on the first try, use the Agent tool to perform the search for you.

When to use the Agent tool:
- If you are searching for a keyword like "config" or "logger", or for questions like "which file does X?", the Agent tool is strongly recommended

When NOT to use the Agent tool:
- If you want to read a specific file path, use the {VIEW_TOOL_NAME} or {GLOB_TOOL_NAME} tool instead of the Agent tool, to find the match more quickly
- If you are searching for a specific class definition like "class Foo", use the {GREP_TOOL_NAME} tool instead, to find the match more quickly
- If you are searching for code within a specific file or set of 2-3 files, use the {GREP_TOOL_NAME} tool instead, to find the match more quickly

Usage notes:
1. Launch multiple agents concurrently whenever possible, to maximize performance; to do that, use a single message with multiple tool uses
2. When the agent is done, it will return a single message back to you. The result returned by the agent is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result.
3. Each agent invocation is stateless. You will not be able to send additional messages to the agent, nor will the agent be able to communicate with you outside of its final report. Therefore, your prompt should contain a highly detailed task description for the agent to perform autonomously and you should specify exactly what information the agent should return back to you in its final and only message to you.
4. The agent's outputs should generally be trusted
5. IMPORTANT: The agent can not modify files. If you want to modify files, do it directly instead of going through the agent."#, format_dispatch_agent_tools()),
        name: "dispatch_agent",
        require_approval: false
    }

    async fn run(
        tool: &DispatchAgentTool,
        params: DispatchAgentParams,
        context: &steer_tools::ExecutionContext,
    ) -> std::result::Result<steer_tools::result::AgentResult, ToolError> {
        let token = context.cancellation_token.clone();

        // Load registries for API client - these are lightweight to load
        let model_registry = Arc::new(crate::model_registry::ModelRegistry::load(&[])
            .map_err(|e| ToolError::execution("dispatch_agent", format!("Failed to load model registry: {e}")))?);
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[])
            .map_err(|e| ToolError::execution("dispatch_agent", format!("Failed to load provider registry: {e}")))?);

        let api_client = Arc::new(crate::api::Client::new_with_deps(
            (*tool.llm_config_provider).clone(),
            provider_registry,
            model_registry,
        )); // Create ApiClient and wrap in Arc
        let agent_executor = AgentExecutor::new(api_client);

        let tool_executor = Arc::new(ToolExecutor::with_workspace(tool.workspace.clone()));

        let available_tools: Vec<ToolSchema> = tool_executor.get_tool_schemas().await;
        let tool_approval_callback = move |_tool_call: ToolCall| {
            async move { Ok(ApprovalDecision::Approved) }
        };

        let tool_execution_callback =
            move |tool_call: ToolCall, callback_token: CancellationToken| {
                let executor = tool_executor.clone();
                async move {
                    executor
                        .execute_tool_with_cancellation(&tool_call, callback_token)
                        .await
                }
            };

        // --- Prepare for AgentExecutor ---
        let initial_messages = vec![Message {
            data: MessageData::User {
                content: vec![UserContent::Text { text: params.prompt }],
            },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            parent_message_id: None,
        }];

        let system_prompt = create_dispatch_agent_system_prompt(&tool.workspace)
            .await
            .map_err(|e| ToolError::execution(DISPATCH_AGENT_TOOL_NAME, format!("Failed to create system prompt: {e}")))?;

        // Use a channel to receive events, though we might just aggregate the final result here.
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);

        // --- Run AgentExecutor ---
        let operation_result = agent_executor
            .run(
                AgentExecutorRunRequest
                 {
                    model: crate::config::model::builtin::claude_3_7_sonnet_20250219(), // Or make configurable?
                    initial_messages,
                    system_prompt: Some(system_prompt),
                    available_tools,
                    tool_approval_callback,
                    tool_execution_callback,
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
            if let AgentEvent::MessageFinal(msg) = event {
                if final_text.is_empty() {
                    final_text = msg.extract_text();
                }
            }
        }


        match operation_result {
            Ok(message) => {
                 // If we still don't have text, extract from final message object
                 if final_text.is_empty() {
                     final_text = message.extract_text();
                 }
                 Ok(steer_tools::result::AgentResult {
                     content: final_text,
                 })
            }
            Err(e) => {
                 Err(ToolError::execution(DISPATCH_AGENT_TOOL_NAME, e.to_string()))
            }
        }
    }
}

pub async fn create_dispatch_agent_system_prompt(
    workspace: &Arc<dyn crate::workspace::Workspace>,
) -> crate::error::Result<String> {
    // Get full environment context
    let env_info = workspace.environment().await?;
    let env_context = env_info.as_context();

    let dispatch_prompt = format!(
        r#"You are an agent for a CLI-based coding tool. Given the user's prompt, you should use the tools available to you to answer the user's question.

Notes:
1. IMPORTANT: You should be concise, direct, and to the point, since your responses will be displayed on a command line interface. Answer the user's question directly, without elaboration, explanation, or details. One word answers are best. Avoid introductions, conclusions, and explanations. You MUST avoid text before/after your response, such as "The answer is <answer>.", "Here is the content of the file..." or "Based on the information provided, the answer is..." or "Here is what I will do next...".
2. When relevant, share file names and code snippets relevant to the query
3. Any file paths you return in your final response MUST be absolute. DO NOT use relative paths.

{env_context}
"#
    );

    Ok(dispatch_prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotenvy::dotenv;
    use steer_workspace::local::LocalWorkspace;

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

        let auth_storage = Arc::new(crate::test_utils::InMemoryAuthStorage::new());
        let llm_config_provider = Arc::new(LlmConfigProvider::new(auth_storage));

        // Create execution context
        let context = steer_tools::ExecutionContext::new("test_tool_call".to_string())
            .with_working_directory(temp_dir.path().to_path_buf())
            .with_cancellation_token(tokio_util::sync::CancellationToken::new());

        // Test prompt that should search for specific code
        let prompt = "Find all files that contain definitions of functions or methods related to search or find operations. Return only the absolute file path.";

        let params = DispatchAgentParams {
            prompt: prompt.to_string(),
        };

        // Instantiate the tool struct (assuming default if no specific state needed)
        let tool_instance = DispatchAgentTool {
            llm_config_provider,
            workspace: Arc::new(
                LocalWorkspace::with_path(temp_dir.path().to_path_buf())
                    .await
                    .unwrap(),
            ),
        };

        // Execute the agent using the run method
        let result = run(&tool_instance, params, &context).await;

        // Check if we got a valid response
        assert!(result.is_ok(), "Agent execution failed: {:?}", result.err());
        let response = result.unwrap();
        assert!(!response.content.is_empty(), "Response should not be empty");
        assert!(
            response.content.contains("search_code.rs"),
            "Response should contain the file path"
        ); // Check for expected content

        println!("Dispatch agent response: {}", response.content);
        println!("Dispatch agent test passed successfully!");
    }
}
