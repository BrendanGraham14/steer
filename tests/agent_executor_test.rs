#[cfg(test)]
mod tests {
    use coder::app::{AgentEvent, AgentExecutorError, AgentExecutorRunRequest, ApprovalDecision};
    use coder::config::LlmConfig; // Import LlmConfig
    use coder::tools::ToolError; // Add necessary tool imports
    use coder::{
        api::{
            Client as ApiClient, InputSchema, Model,
            messages::{ContentBlock, Message, MessageContent, MessageRole, StructuredContent},
            tools::{Tool, ToolCall},
        },
        app::AgentExecutor,
    }; // Add necessary api imports
    use dotenv::dotenv;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};
    use tokio_util::sync::CancellationToken; // For creating tool input

    // Helper function to create a basic text message
    fn text_message(role: MessageRole, content: &str) -> Message {
        Message {
            id: None,
            role,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![ContentBlock::Text {
                    text: content.to_string(),
                }]),
            },
        }
    }

    // Helper to get a real client (requires env vars)
    fn get_real_client() -> Arc<ApiClient> {
        dotenv().ok(); // Load .env file if present
        let config = LlmConfig::from_env().expect("LLM config failed to load");
        Arc::new(ApiClient::new(&config))
    }

    // Test Case 1: Basic text response without tools
    #[tokio::test]
    #[ignore] // Ignored because it makes real API calls
    async fn test_run_operation_basic_text_response_real() {
        let client = get_real_client();
        let executor = AgentExecutor::new(client.clone());
        let model = Model::Gpt4_1Nano20250414; // Use a fast model
        let initial_messages = vec![text_message(MessageRole::User, "Hello, world!")];
        let system_prompt = Some("You are a test assistant. Respond concisely.".to_string());
        let available_tools: Vec<Tool> = vec![];
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let token = CancellationToken::new();
        // Dummy tool executor callback (shouldn't be called)
        let tool_executor_callback = |_call: ToolCall, _token: CancellationToken| async {
            panic!("Tool executor should not be called");
            #[allow(unreachable_code)]
            Ok::<String, ToolError>("".to_string())
        };

        let final_message_result = executor
            .run(
                AgentExecutorRunRequest {
                    model,
                    initial_messages,
                    system_prompt,
                    available_tools,
                    tool_executor_callback,
                },
                event_tx,
                token,
            )
            .await;

        // Basic assertions for real calls
        assert!(final_message_result.is_ok());
        let final_message = final_message_result.unwrap();
        assert_eq!(final_message.role, MessageRole::Assistant);
        match &final_message.content {
            MessageContent::Text { content } => assert!(!content.is_empty()),
            MessageContent::StructuredContent { content } => assert!(!content.0.is_empty()),
        }; // Should have *some* content

        // Check events (expect at least one part and one final)
        let mut has_part = false;
        let mut has_final = false;
        while let Ok(Some(event)) = timeout(Duration::from_secs(1), event_rx.recv()).await {
            match event {
                AgentEvent::AssistantMessagePart(_) => has_part = true,
                AgentEvent::AssistantMessageFinal(_) => has_final = true,
                _ => {}
            }
        }
        assert!(
            has_part
                || match &final_message.content {
                    MessageContent::Text { content } => !content.is_empty(),
                    MessageContent::StructuredContent { content } => !content.0.is_empty(),
                }
        ); // Ensure we received text either via parts or final message
        assert!(has_final);
    }

    // Test Case 2: Automatic Tool Execution (Success) - More complex for real API
    #[tokio::test]
    #[ignore] // Ignored because it makes real API calls
    async fn test_run_operation_auto_tool_success_real() {
        let client = get_real_client();
        let executor = AgentExecutor::new(client.clone());
        let model = Model::Gpt4_1Nano20250414; // Use a fast model supporting tools
        let initial_messages = vec![text_message(
            MessageRole::User,
            "What is the capital of France?",
        )];
        // Provide a dummy tool definition that the LLM might try to call
        let available_tools = vec![Tool {
            name: "get_capital".to_string(),
            description: "Gets the capital of a country".to_string(),
            input_schema: InputSchema {
                properties: json!({
                    "country": { "type": "string", "description": "The country name" }
                })
                .as_object()
                .unwrap()
                .clone(),
                required: vec!["country".to_string()],
                schema_type: "object".to_string(),
            },
        }];
        let (event_tx, mut event_rx) = mpsc::channel(20);
        let token = CancellationToken::new();

        // Tool executor callback - expects get_capital
        let tool_executor_callback = move |call: ToolCall, _token: CancellationToken| async move {
            if call.name == "get_capital" {
                let input_country = call.parameters.get("country").and_then(|v| v.as_str());
                if input_country == Some("France") {
                    Ok("Paris".to_string())
                } else {
                    Err(ToolError::Execution {
                        tool_name: call.name.clone(),
                        message: format!("Unexpected country: {:?}", input_country),
                    })
                }
            } else {
                Err(ToolError::UnknownTool(call.name.clone()))
            }
        };

        let final_message_result = executor
            .run(
                AgentExecutorRunRequest {
                    model,
                    initial_messages,
                    system_prompt: Some("You are a helpful assistant.".to_string()),
                    available_tools,
                    tool_executor_callback,
                },
                event_tx,
                token,
            )
            .await;

        // --- Assertions ---
        assert!(final_message_result.is_ok());
        let final_message = final_message_result.unwrap();
        assert_eq!(final_message.role, MessageRole::Assistant);
        // Check if the response contains "Paris" (case-insensitive)
        assert!(
            final_message
                .content
                .extract_text()
                .to_lowercase()
                .contains("paris")
        );

        // Check Events - Look for key events, order might vary slightly
        let mut saw_final_with_tool_call = false;
        let mut saw_executing = false;
        let mut saw_tool_result = false;
        let mut saw_final_text = false;
        while let Ok(Some(event)) = timeout(Duration::from_secs(5), event_rx.recv()).await {
            match event {
                AgentEvent::AssistantMessageFinal(msg) => {
                    // Check if we have tool calls in the structured content
                    if let MessageContent::StructuredContent { content } = &msg.content {
                        if content.0.iter().any(|block| {
                            matches!(block, coder::api::messages::ContentBlock::ToolUse { .. })
                        }) {
                            saw_final_with_tool_call = true;
                        } else {
                            // If we have text in structured content without tool calls, consider it final text
                            saw_final_text = true;
                        }
                    }
                }
                AgentEvent::ExecutingTool {
                    tool_call_id: _,
                    name,
                } => {
                    if name == "get_capital" {
                        saw_executing = true;
                    }
                }
                AgentEvent::ToolResultReceived(res) => {
                    // Log all tool results for debugging
                    println!(
                        "Test: Received ToolResultReceived event with ID: {}, output: {}, is_error: {}",
                        res.tool_call_id, res.output, res.is_error
                    );

                    // More permissive matching - the ID might not start with tool_
                    // and we're just checking for the expected output
                    if res.output == "Paris" && !res.is_error {
                        println!("Test: Matched correct tool result with output 'Paris'");
                        saw_tool_result = true;
                    }
                }
                _ => {}
            }
        }

        assert!(
            saw_final_with_tool_call,
            "Did not see final message requesting tool"
        );
        assert!(
            saw_executing,
            "Did not see ExecutingTool event for get_capital"
        );
        assert!(
            saw_tool_result,
            "Did not see correct ToolResultReceived event"
        );
        assert!(
            saw_final_text,
            "Did not see final text message after tool use"
        );
    }
}
