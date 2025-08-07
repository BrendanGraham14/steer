#[cfg(test)]
mod tests {
    use steer_core::api::Client;
    use steer_core::app::conversation::{AssistantContent, Message, Role, UserContent};
    use steer_core::app::{
        AgentEvent, AgentExecutor, AgentExecutorRunRequest, ApprovalDecision, MessageData,
    };

    use dotenvy::dotenv;
    use serde_json::json;
    use std::sync::Arc;
    use steer_core::test_utils;
    use steer_tools::{
        InputSchema, ToolCall, ToolSchema as Tool,
        result::{ExternalResult, ToolResult},
    };
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};
    use tokio_util::sync::CancellationToken;

    // Helper function to create a basic text message
    fn text_message(role: &str, content: &str) -> Message {
        let timestamp = Message::current_timestamp();

        match role {
            "user" => Message {
                data: MessageData::User {
                    content: vec![UserContent::Text {
                        text: content.to_string(),
                    }],
                },
                timestamp,
                id: Message::generate_id("user", timestamp),
                parent_message_id: None,
            },
            "assistant" => Message {
                data: MessageData::Assistant {
                    content: vec![AssistantContent::Text {
                        text: content.to_string(),
                    }],
                },
                timestamp,
                id: Message::generate_id("assistant", timestamp),
                parent_message_id: None,
            },
            _ => unreachable!("Invalid role: {role}"),
        }
    }

    // Helper to get a real client (requires env vars)
    async fn get_real_client() -> Arc<Client> {
        dotenv().ok(); // Load .env file if present
        let provider = test_utils::test_llm_config_provider();
        Arc::new(Client::new_with_provider(provider))
    }

    // Test Case 1: Basic text response without tools
    #[tokio::test]
    #[ignore] // Ignored because it makes real API calls
    async fn test_run_operation_basic_text_response_real() {
        let client = get_real_client().await;
        let executor = AgentExecutor::new(client.clone());
        let model = steer_core::config::model::builtin::gpt_4_1_mini_2025_04_14(); // Use a fast model
        let initial_messages = vec![text_message("user", "Hello, world!")];
        let system_prompt = Some("You are a test assistant. Respond concisely.".to_string());
        let available_tools: Vec<Tool> = vec![];
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let token = CancellationToken::new();
        // Dummy tool executor callback (shouldn't be called)
        let tool_approval_callback = |_call: ToolCall| async {
            unreachable!("Tool approval should not be called");
        };
        let tool_execution_callback = |_call: ToolCall, _token: CancellationToken| async {
            unreachable!("Tool executor should not be called");
        };

        let final_message_result = executor
            .run(
                AgentExecutorRunRequest {
                    model,
                    initial_messages,
                    system_prompt,
                    available_tools,
                    tool_approval_callback,
                    tool_execution_callback,
                },
                event_tx,
                token,
            )
            .await;

        // Basic assertions for real calls
        assert!(final_message_result.is_ok());
        let final_message = final_message_result.unwrap();
        assert_eq!(final_message.role(), Role::Assistant);
        assert!(matches!(&final_message.data, MessageData::Assistant { .. }));
        match &final_message.data {
            MessageData::Assistant { content, .. } => {
                assert!(!content.is_empty());
            }
            _ => unreachable!(),
        }; // Should have *some* content

        // Check events (expect at least one part and one final)
        let has_part = false;
        let mut has_final = false;
        while let Ok(Some(event)) = timeout(Duration::from_secs(1), event_rx.recv()).await {
            if let AgentEvent::MessageFinal(_) = event {
                has_final = true
            }
        }
        assert!(
            has_part
                || match &final_message.data {
                    MessageData::Assistant { content, .. } => !content.is_empty(),
                    _ => false,
                }
        ); // Ensure we received text either via parts or final message
        assert!(has_final);
    }

    // Test Case 2: Automatic Tool Execution (Success) - More complex for real API
    #[tokio::test]
    #[ignore] // Ignored because it makes real API calls
    async fn test_run_operation_auto_tool_success_real() {
        let client = get_real_client().await;
        let executor = AgentExecutor::new(client.clone());
        let model = steer_core::config::model::builtin::gpt_4_1_mini_2025_04_14(); // Use a fast model supporting tools
        let initial_messages = vec![text_message("user", "What is the capital of France?")];
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

        // Tool approval callback - always approve
        let tool_approval_callback =
            move |_call: ToolCall| async move { Ok(ApprovalDecision::Approved) };

        // Tool executor callback - expects get_capital
        let tool_execution_callback = move |call: ToolCall, _token: CancellationToken| async move {
            if call.name == "get_capital" {
                let input_country = call.parameters.get("country").and_then(|v| v.as_str());
                if input_country == Some("France") {
                    Ok(ToolResult::External(ExternalResult {
                        tool_name: call.name.clone(),
                        payload: "Paris".to_string(),
                    }))
                } else {
                    Err(steer_tools::ToolError::Execution {
                        tool_name: call.name.clone(),
                        message: format!("Unexpected country: {input_country:?}"),
                    })
                }
            } else {
                Err(steer_tools::ToolError::UnknownTool(call.name.clone()))
            }
        };

        let final_message_result = executor
            .run(
                AgentExecutorRunRequest {
                    model,
                    initial_messages,
                    system_prompt: Some("You are a helpful assistant.".to_string()),
                    available_tools,
                    tool_approval_callback,
                    tool_execution_callback,
                },
                event_tx,
                token,
            )
            .await;

        // --- Assertions ---
        assert!(final_message_result.is_ok());
        let final_message = final_message_result.unwrap();
        assert_eq!(final_message.role(), Role::Assistant);
        // Check if the response contains "Paris" (case-insensitive)
        assert!(matches!(&final_message.data, MessageData::Assistant { .. }));
        let response_text = match &final_message.data {
            MessageData::Assistant { content, .. } => content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
            _ => unreachable!(),
        };
        assert!(
            response_text.to_lowercase().contains("paris"),
            "Response should contain 'Paris'"
        );

        // Check Events - Look for key events, order might vary slightly
        let mut saw_final_with_tool_call = false;
        let mut saw_executing = false;
        let mut saw_tool_result = false;
        let mut saw_final_text = false;
        while let Ok(Some(event)) = timeout(Duration::from_secs(5), event_rx.recv()).await {
            match event {
                AgentEvent::MessageFinal(message) => {
                    match message.data {
                        MessageData::Assistant { content, .. } => {
                            if content
                                .iter()
                                .any(|c| matches!(c, AssistantContent::ToolCall { .. }))
                            {
                                saw_final_with_tool_call = true;
                            } else {
                                saw_final_text = true;
                            }
                        }
                        MessageData::Tool {
                            tool_use_id,
                            result,
                        } => {
                            println!(
                                "Test: Received ToolResultReceived event with ID: {tool_use_id}, result: {result:?}"
                            );

                            // Check if we got the expected result
                            if let ToolResult::External(ext_result) = &result {
                                if ext_result.payload == "Paris" {
                                    println!(
                                        "Test: Matched correct tool result with output 'Paris'"
                                    );
                                    saw_tool_result = true;
                                }
                            }
                        }
                        _ => {}
                    }
                }

                AgentEvent::ExecutingTool {
                    tool_call_id: _,
                    name,
                    parameters: _,
                } => {
                    if name == "get_capital" {
                        saw_executing = true;
                    }
                }
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
