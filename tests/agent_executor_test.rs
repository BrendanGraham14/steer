#[cfg(test)]
mod tests {
    use coder::app::{AgentEvent, AgentExecutorError, ApprovalDecision, ApprovalMode};
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
                model,
                initial_messages,
                system_prompt,
                available_tools,
                tool_executor_callback,
                event_tx,
                ApprovalMode::Automatic,
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
                model,
                initial_messages,
                Some("You are a helpful assistant.".to_string()),
                available_tools,
                tool_executor_callback,
                event_tx,
                ApprovalMode::Automatic,
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

    // --- Test Case: Interactive Tool Approval (Approved) ---
    #[tokio::test]
    #[ignore] // Requires real API calls and interaction simulation
    async fn test_run_operation_interactive_tool_approved_real() {
        // --- Arrange ---
        let client = get_real_client();
        let executor = AgentExecutor::new(client.clone());
        let model = Model::Gpt4_1Nano20250414;
        let initial_messages = vec![text_message(
            MessageRole::User,
            "What is the weather like in London? Use the 'get_weather' tool.",
        )];
        let tool_name = "get_weather";
        let tool_output = "Sunny, 25°C"; // Expected output from our callback

        let available_tools = vec![Tool {
            name: tool_name.to_string(),
            description: "Gets the current weather for a location".to_string(),
            input_schema: InputSchema {
                properties:
                    json!({ "location": { "type": "string", "description": "The city name" } })
                        .as_object()
                        .unwrap()
                        .clone(),
                required: vec!["location".to_string()],
                schema_type: "object".to_string(),
            },
        }];
        let (event_tx, mut event_rx) = mpsc::channel(20);
        let token = CancellationToken::new();
        let tool_executed = Arc::new(std::sync::Mutex::new(false));

        // Tool executor callback
        let tool_executed_clone = tool_executed.clone();
        let tool_executor_callback = move |call: ToolCall, _token: CancellationToken| {
            let executed = tool_executed_clone.clone();
            async move {
                if call.name == tool_name {
                    // Verify input loosely (LLM might add details)
                    if let Some(location) = call.parameters.get("location").and_then(|v| v.as_str())
                    {
                        assert!(
                            location.to_lowercase().contains("london"),
                            "Location should contain London"
                        );
                    }
                    *executed.lock().unwrap() = true;
                    Ok(tool_output.to_string())
                } else {
                    Err(ToolError::UnknownTool(call.name.clone()))
                }
            }
        };

        // --- Act ---
        let executor_handle = tokio::spawn(async move {
            executor
                .run(
                    model,
                    initial_messages,
                    Some("You must use provided tools when asked.".to_string()),
                    available_tools,
                    tool_executor_callback,
                    event_tx,
                    ApprovalMode::Interactive, // <<< Interactive
                    token,
                )
                .await
        });

        // --- Assert Events and Handle Approval ---
        let mut saw_request_approval = false;
        let mut saw_executing_tool = false;
        let mut saw_tool_result = false;
        let mut saw_final_message_text = false;
        let mut received_tool_call_id = None; // Store the ID when received

        loop {
            tokio::select! {
                biased; // Prioritize timeout
                _ = tokio::time::sleep(Duration::from_secs(30)) => { // Generous timeout
                    panic!("Test timed out waiting for events or completion");
                }
                maybe_event = event_rx.recv() => {
                    match maybe_event {
                        Some(AgentEvent::RequestToolApprovals(req_event)) => {
                            println!("Test: Received RequestToolApprovals");
                            saw_request_approval = true;
                            assert_eq!(req_event.len(), 1, "Expected exactly one tool request");

                            // Process the first request directly
                            let request = req_event.into_iter().next().unwrap();
                            let tool_call = request.tool_call;
                            let responder = request.responder;
                            let received_tool_call_id_clone = received_tool_call_id.clone(); // Capture for assertion

                            assert_eq!(tool_call.name, tool_name);
                            assert!(tool_call.parameters.get("location").is_some());
                            // Store the ID for later checks
                            let tool_call_id_local = tool_call.id.clone();
                            if received_tool_call_id_clone.is_some() {
                                assert_eq!(received_tool_call_id_clone.as_ref(), Some(&tool_call_id_local),
                                    "Tool call ID in request event mismatch");
                            }

                            // Approve the tool directly via the oneshot sender
                            println!("Test: Approving tool call ID {}", tool_call_id_local);
                            responder.send(ApprovalDecision::Approved).unwrap();
                        },
                        Some(AgentEvent::ExecutingTool{ tool_call_id: id, name }) => {
                            println!("Test: Received ExecutingTool");
                            assert_eq!(name, tool_name);
                            if received_tool_call_id.is_some() {
                                assert_eq!(received_tool_call_id.as_ref(), Some(&id), "ExecutingTool ID mismatch");
                            } else {
                                received_tool_call_id = Some(id.clone());
                            }
                            saw_executing_tool = true;
                        },
                        Some(AgentEvent::ToolResultReceived(result)) => {
                            println!("Test: Received ToolResultReceived");
                            if received_tool_call_id.is_some() {
                                assert_eq!(received_tool_call_id.as_ref(), Some(&result.tool_call_id),
                                    "ToolResultReceived ID mismatch");
                            }
                            assert!(!result.is_error);
                            assert_eq!(result.output, tool_output);
                            saw_tool_result = true;
                        },
                        Some(AgentEvent::AssistantMessageFinal(msg)) => {
                            println!("Test: Received AssistantMessageFinal");
                            let extracted_text = msg.content.extract_text();
                            let is_tool_use = matches!(&msg.content, MessageContent::StructuredContent { content }
                                if content.0.iter().any(|block| matches!(block, coder::api::messages::ContentBlock::ToolUse { .. })));

                            // Log the content for debugging
                            println!("Test: Message content: {}", extracted_text);

                            if is_tool_use {
                                // Store the tool call ID from the message
                                if let MessageContent::StructuredContent { content } = msg.content {
                                    if let Some(coder::api::messages::ContentBlock::ToolUse { id, name: msg_tool_name, .. }) =
                                        content.0.iter().find(|b| matches!(b, coder::api::messages::ContentBlock::ToolUse{..})) {
                                        assert_eq!(msg_tool_name, tool_name);
                                        received_tool_call_id = Some(id.clone());
                                        println!("Test: Stored tool call ID from message: {}", id);
                                    }
                                }
                            } else {
                                // We potentially received a non-tool call message - check if it could be a final message
                                if extracted_text.to_lowercase().contains("sunny") ||
                                   extracted_text.to_lowercase().contains("weather") ||
                                   extracted_text.to_lowercase().contains("london") ||
                                   extracted_text.to_lowercase().contains("25°c") {
                                   println!("Test: Found final message text with weather info");
                                   saw_final_message_text = true;
                               }
                            }
                        },
                        Some(AgentEvent::AssistantMessagePart(_)) => {} // Ignore parts
                        None => break, // Channel closed
                        e => eprintln!("Test: Unexpected event: {:?}", e),
                    }
                },
            }
            // Exit condition: saw final message *after* requesting approval
            if saw_final_message_text && saw_request_approval {
                break;
            }
        }

        let final_result = executor_handle.await.unwrap();

        // --- Final Assertions ---
        assert!(
            final_result.is_ok(),
            "Operation failed: {:?}",
            final_result.err()
        );
        assert!(
            saw_request_approval,
            "Did not receive RequestToolApprovals event"
        );
        assert!(saw_executing_tool, "Did not receive ExecutingTool event");
        assert!(saw_tool_result, "Did not receive ToolResultReceived event");
        assert!(
            saw_final_message_text,
            "Did not receive final message text event"
        );
        assert!(
            *tool_executed.lock().unwrap(),
            "Tool executor callback was not called"
        );

        let final_message = final_result.unwrap();
        // Log the final message for debugging
        let final_text = final_message.content.extract_text();
        println!("Test: Final message text: {}", final_text);

        // Check for weather-related keywords in a more flexible way
        let final_text_lower = final_text.to_lowercase();
        assert!(
            final_text_lower.contains("london")
                || final_text_lower.contains("weather")
                || final_text_lower.contains("sunny")
                || final_text_lower.contains("25°c")
                || final_text_lower.contains("degrees"),
            "Final message should contain weather information: {}",
            final_text
        );
    }

    // --- Test Case: Interactive Tool Approval (Denied) ---
    #[tokio::test]
    #[ignore] // Requires real API calls and interaction simulation
    async fn test_run_operation_interactive_tool_denied_real() {
        // --- Arrange ---
        let client = get_real_client();
        let executor = AgentExecutor::new(client.clone());
        let model = Model::Gpt4_1Nano20250414;
        let initial_messages = vec![text_message(
            MessageRole::User,
            "Please use the 'dangerous_tool' to do something.",
        )];
        let tool_name = "dangerous_tool";
        let denial_reason = "Tool execution denied by user.";

        let available_tools = vec![Tool {
            name: tool_name.to_string(),
            description: "A tool that requires explicit user approval".to_string(),
            input_schema: InputSchema {
                properties: json!({"action": {"type": "string"}})
                    .as_object()
                    .unwrap()
                    .clone(),
                required: vec!["action".to_string()],
                schema_type: "object".to_string(),
            },
        }];
        let (event_tx, mut event_rx) = mpsc::channel(20);
        let token = CancellationToken::new();
        let tool_executed = Arc::new(std::sync::Mutex::new(false)); // Should remain false

        // Tool executor callback (should NOT be called)
        let tool_executed_clone = tool_executed.clone();
        let tool_executor_callback = move |call: ToolCall, _token: CancellationToken| {
            let executed = tool_executed_clone.clone();
            async move {
                *executed.lock().unwrap() = true; // Mark if called (shouldn't be)
                panic!(
                    "Tool executor should not be called for denied tool: {}",
                    call.name
                );
                #[allow(unreachable_code)]
                Err::<String, ToolError>(ToolError::UnknownTool(call.name.clone()))
            }
        };

        // --- Act ---
        let executor_handle = tokio::spawn(async move {
            executor
                .run(
                    model,
                    initial_messages,
                    Some(
                        "You must use provided tools when asked. Confirm dangerous actions."
                            .to_string(),
                    ),
                    available_tools,
                    tool_executor_callback,
                    event_tx,
                    ApprovalMode::Interactive, // <<< Interactive
                    token,
                )
                .await
        });

        // --- Assert Events and Handle Denial ---
        let mut saw_request_approval = false;
        let mut saw_tool_result_denied = false;
        let mut saw_final_message_text = false;
        let mut received_tool_call_id = None;

        loop {
            tokio::select! {
                biased;
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    panic!("Test timed out waiting for events or completion");
                }
                maybe_event = event_rx.recv() => {
                    match maybe_event {
                        Some(AgentEvent::RequestToolApprovals(req_event)) => {
                            println!("Test: Received RequestToolApprovals");
                            saw_request_approval = true;
                            assert_eq!(req_event.len(), 1, "Expected exactly one tool request");
                            let received_tool_call_id_clone = received_tool_call_id.clone();

                            // Process the first request directly
                            let request = req_event.into_iter().next().unwrap();
                            let tool_call = request.tool_call;
                            let responder = request.responder;

                            let tool_call_id_local = tool_call.id.clone();
                            if received_tool_call_id_clone.is_some() {
                                assert_eq!(received_tool_call_id_clone.as_ref(), Some(&tool_call_id_local));
                            }
                            assert_eq!(tool_call.name, tool_name);
                            // Deny the tool directly via the oneshot sender
                            println!("Test: Denying tool call ID {}", tool_call_id_local);
                            responder.send(ApprovalDecision::Denied).unwrap(); // <<< Deny
                        },
                        Some(AgentEvent::ExecutingTool{ .. }) => {
                            panic!("ExecutingTool event received for a denied tool");
                        },
                        Some(AgentEvent::ToolResultReceived(result)) => {
                            println!("Test: Received ToolResultReceived");
                            if received_tool_call_id.is_some() {
                                assert_eq!(received_tool_call_id.as_ref(), Some(&result.tool_call_id),
                                    "ToolResultDenied ID mismatch");
                            }
                            assert!(result.is_error); // Should be an error result
                            assert!(result.output.contains(denial_reason));
                            saw_tool_result_denied = true;
                        },
                        Some(AgentEvent::AssistantMessageFinal(msg)) => {
                            println!("Test: Received AssistantMessageFinal");
                            let extracted_text = msg.content.extract_text();
                            let is_tool_use = matches!(&msg.content, MessageContent::StructuredContent { content }
                                if content.0.iter().any(|block| matches!(block, coder::api::messages::ContentBlock::ToolUse { .. })));

                            // Log the content for debugging
                            println!("Test: Denied tool message content: {}", extracted_text);
                            println!("extracted_text: {}",extracted_text);
                            if is_tool_use {
                                // Store the tool call ID from the message
                                if let MessageContent::StructuredContent { content } = msg.content {
                                    if let Some(coder::api::messages::ContentBlock::ToolUse { id, name: msg_tool_name, .. }) =
                                        content.0.iter().find(|b| matches!(b, coder::api::messages::ContentBlock::ToolUse{..})) {
                                        assert_eq!(msg_tool_name, tool_name);
                                        received_tool_call_id = Some(id.clone());
                                        println!("Test: Stored tool call ID from message: {}", id);
                                    }
                                }
                            } else {
                                // More flexible detection of final message
                                let lower_text = extracted_text.to_lowercase();
                                if lower_text.contains("okay") ||
                                   lower_text.contains("understand") ||
                                   lower_text.contains("cannot") ||
                                   lower_text.contains("sorry") ||
                                   lower_text.contains("denied") ||
                                   lower_text.contains("unable")  {
                                   println!("Test: Found final message acknowledging denial");
                                   saw_final_message_text = true;
                                }
                            }
                        },
                        Some(AgentEvent::AssistantMessagePart(_)) => {} // Ignore
                        None => break, // Channel closed
                        e => eprintln!("Test: Unexpected event: {:?}", e),
                    }
                },
            }
            // Exit condition
            if saw_final_message_text && saw_request_approval {
                break;
            }
        }

        let final_result = executor_handle.await.unwrap();

        // --- Final Assertions ---
        assert!(
            final_result.is_ok(),
            "Operation failed: {:?}",
            final_result.err()
        ); // LLM should still respond
        assert!(
            saw_request_approval,
            "Did not receive RequestToolApprovals event"
        );
        assert!(
            saw_tool_result_denied,
            "Did not receive correct ToolResultReceived event for denial"
        );
        assert!(
            saw_final_message_text,
            "Did not receive final message text event"
        );
        assert!(
            !*tool_executed.lock().unwrap(),
            "Tool executor callback was called for denied tool"
        );

        let final_message = final_result.unwrap();
        // Check that the final message indicates understanding/acknowledgement of denial
        // Log the final message for debugging
        let final_text = final_message.content.extract_text();
        println!("Test: Final message text for denied tool: {}", final_text);

        let final_text_lower = final_text.to_lowercase();
        assert!(
            final_text_lower.contains("okay")
                || final_text_lower.contains("understand")
                || final_text_lower.contains("cannot")
                || final_text_lower.contains("sorry")
                || final_text_lower.contains("not")
                || final_text_lower.contains("denied")
                || final_text_lower.contains("unable"),
            "Final message should acknowledge the denial in some way: {}",
            final_text
        );
    }

    // --- Test Case: Cancellation During Tool Execution (Automatic Mode) ---
    #[tokio::test]
    #[ignore] // Requires real API calls and timing sensitivity
    async fn test_run_operation_cancel_during_tool_execution_auto_real() {
        // --- Arrange ---
        let client = get_real_client();
        let executor = AgentExecutor::new(client.clone());
        let model = Model::Gpt4_1Nano20250414;
        let initial_messages = vec![text_message(
            MessageRole::User,
            "Use the 'long_running_task' tool.",
        )];
        let tool_name = "long_running_task";

        let available_tools = vec![Tool {
            name: tool_name.to_string(),
            description: "A task that takes a few seconds to complete".to_string(),
            input_schema: InputSchema {
                properties: json!({}).as_object().unwrap().clone(),
                required: vec![],
                schema_type: "object".to_string(),
            },
        }];
        let (event_tx, mut event_rx) = mpsc::channel(20);
        let token = CancellationToken::new();
        let (tool_start_tx, mut tool_start_rx) = mpsc::channel::<()>(1); // Signal tool start
        let tool_cancelled_correctly = Arc::new(std::sync::Mutex::new(false));
        let mut received_tool_call_id = None; // Store ID

        // Tool executor callback that waits and checks for cancellation
        let tool_cancelled_clone = tool_cancelled_correctly.clone();
        let tool_executor_callback = move |call: ToolCall, tool_token: CancellationToken| {
            let started_tx = tool_start_tx.clone();
            let cancelled_flag = tool_cancelled_clone.clone();
            async move {
                println!("Test Tool: Started {}", call.name);
                started_tx.send(()).await.ok(); // Signal that tool execution has begun

                // Wait for cancellation or timeout (shorter for testing)
                tokio::select! {
                    _ = tool_token.cancelled() => {
                        println!("Test Tool: Cancellation detected");
                        *cancelled_flag.lock().unwrap() = true;
                        Err(ToolError::Cancelled(call.name))
                    },
                    // Reduce sleep to make test faster, but long enough to be cancellable
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        panic!("Tool did not get cancelled in time");
                    }
                }
            }
        };

        // --- Act ---
        let token_clone = token.clone();
        let executor_handle = tokio::spawn(async move {
            executor
                .run(
                    model,
                    initial_messages,
                    Some("Use tools when requested.".to_string()),
                    available_tools,
                    tool_executor_callback,
                    event_tx,
                    ApprovalMode::Automatic, // <<< Auto mode
                    token_clone,
                )
                .await
        });

        // --- Assert Events and Trigger Cancellation ---
        let mut saw_executing_tool = false;

        // 1. Wait for the LLM to request the tool
        println!("Test: Waiting for LLM to request tool...");
        while received_tool_call_id.is_none() {
            if let Ok(Some(event)) = timeout(Duration::from_secs(15), event_rx.recv()).await {
                if let AgentEvent::AssistantMessageFinal(msg) = event {
                    if let MessageContent::StructuredContent { content } = &msg.content {
                        if let Some(coder::api::messages::ContentBlock::ToolUse {
                            id,
                            name: msg_tool_name,
                            ..
                        }) = content.0.iter().find(|b| {
                            matches!(b, coder::api::messages::ContentBlock::ToolUse { .. })
                        }) {
                            assert_eq!(msg_tool_name, tool_name);
                            received_tool_call_id = Some(id.clone());
                            println!("Test: LLM requested tool with ID: {}", id);
                            break;
                        }
                    }
                }
            } else {
                panic!("Timed out waiting for LLM tool request");
            }
        }
        let tool_call_id = received_tool_call_id.expect("Tool call ID not received");

        // 2. Wait for the tool to start execution (signalled by tool_start_rx)
        println!("Test: Waiting for tool execution to start...");
        let _ = timeout(Duration::from_secs(10), tool_start_rx.recv())
            .await
            .expect("Timed out waiting for tool execution signal");
        println!("Test: Tool has started execution signal received.");

        // 3. Receive the ExecutingTool event (might arrive slightly before or after signal)
        println!("Test: Waiting for ExecutingTool event...");
        while !saw_executing_tool {
            if let Ok(Some(event)) = timeout(Duration::from_secs(5), event_rx.recv()).await {
                match event {
                    AgentEvent::ExecutingTool {
                        tool_call_id: id,
                        name,
                    } => {
                        assert_eq!(id, tool_call_id);
                        assert_eq!(name, tool_name);
                        saw_executing_tool = true;
                        println!("Test: Saw ExecutingTool event. Cancelling now.");
                        break;
                    }
                    _ => {} // Ignore other events while waiting for this one
                }
            } else {
                panic!("Timed out waiting for ExecutingTool event after tool start signal");
            }
        }

        // 4. Cancel the operation
        token.cancel();

        // --- Final Assertions ---
        let final_result = timeout(Duration::from_secs(10), executor_handle) // Give cancellation time
            .await
            .expect("Executor task did not finish after cancellation")
            .unwrap(); // Unwrap JoinHandle result

        // Check the specific error type
        match final_result {
            // Cancellation can manifest as Cancelled or ToolError::Cancelled
            Err(AgentExecutorError::Cancelled) => {
                println!("Test: Operation correctly resulted in Cancelled error.");
            }
            Err(AgentExecutorError::Tool(ToolError::Cancelled(cancelled_tool_name))) => {
                assert_eq!(cancelled_tool_name, tool_name);
                println!("Test: Operation correctly resulted in ToolError::Cancelled.");
            }
            Ok(_) => panic!("Expected operation to fail due to cancellation, but it succeeded."),
            Err(e) => panic!("Expected Cancelled or ToolError::Cancelled, got {:?}", e),
        }

        assert!(
            *tool_cancelled_correctly.lock().unwrap(),
            "Tool callback did not detect cancellation"
        );

        // Check for the ToolResultReceived event indicating cancellation (might not always be sent if cancellation is very fast)
        // Let's make this optional or check for either Cancelled error or the event.
        let mut saw_cancelled_tool_result = false;
        while let Ok(event) = event_rx.try_recv() {
            if let AgentEvent::ToolResultReceived(result) = event {
                if result.tool_call_id == tool_call_id
                    && result.is_error
                    && (result.output.contains("cancel") || result.output.contains("Cancel"))
                {
                    saw_cancelled_tool_result = true;
                    break;
                }
            }
        }
        // Not asserting strictly - this may not always happen depending on timing:
        // assert!(saw_cancelled_tool_result, "Did not receive cancelled ToolResultReceived event");
        println!(
            "Test: Saw cancelled tool result event: {}",
            saw_cancelled_tool_result
        );
    }

    // --- Test Case: Cancellation During Interactive Approval Wait ---
    #[tokio::test]
    #[ignore] // Requires real API calls and interaction simulation
    async fn test_run_operation_cancel_during_interactive_wait_real() {
        // --- Arrange ---
        let client = get_real_client();
        let executor = AgentExecutor::new(client.clone());
        let model = Model::Gpt4_1Nano20250414;
        let initial_messages = vec![text_message(
            MessageRole::User,
            "Use the 'wait_for_approval' tool.",
        )];
        let tool_name = "wait_for_approval";

        let available_tools = vec![Tool {
            name: tool_name.to_string(),
            description: "A tool that waits for approval".to_string(),
            input_schema: InputSchema {
                properties: json!({}).as_object().unwrap().clone(),
                required: vec![],
                schema_type: "object".to_string(),
            },
        }];
        let (event_tx, mut event_rx) = mpsc::channel(20);
        let token = CancellationToken::new();

        // Tool executor callback (should NOT be called)
        let tool_executor_callback = move |call: ToolCall, _token: CancellationToken| async move {
            panic!(
                "Tool executor should not be called when cancelled during approval: {}",
                call.name
            );
            #[allow(unreachable_code)]
            Err::<String, ToolError>(ToolError::UnknownTool(call.name.clone()))
        };

        // --- Act ---
        let token_clone = token.clone();
        let executor_handle = tokio::spawn(async move {
            executor
                .run(
                    model,
                    initial_messages,
                    Some("Use tools when requested.".to_string()),
                    available_tools,
                    tool_executor_callback,
                    event_tx,
                    ApprovalMode::Interactive, // <<< Interactive
                    token_clone,
                )
                .await
        });

        // --- Assert Events and Trigger Cancellation ---
        let mut saw_request_approval = false;

        // 1. Wait for the RequestToolApprovals event
        println!("Test: Waiting for RequestToolApprovals event...");
        loop {
            if let Ok(Some(event)) = timeout(Duration::from_secs(15), event_rx.recv()).await {
                if let AgentEvent::RequestToolApprovals(req_event) = event {
                    println!("Test: Received RequestToolApprovals event. Cancelling now.");
                    saw_request_approval = true;
                    // Don't interact with channels, just cancel
                    token.cancel();

                    // Drop the requests vector immediately to simulate cancellation during wait
                    // Dropping the vector drops the contained responders (oneshot::Sender)
                    // which signals cancellation to the executor waiting on the receiver.
                    drop(req_event);

                    break;
                }
            } else {
                panic!("Timed out waiting for RequestToolApprovals event");
            }
        }

        // --- Final Assertions ---
        let final_result = timeout(Duration::from_secs(10), executor_handle)
            .await
            .expect("Executor task did not finish after cancellation")
            .unwrap(); // Unwrap JoinHandle result

        assert!(
            saw_request_approval,
            "Did not receive RequestToolApprovals event before cancellation"
        );

        // Expect a Cancellation error
        match final_result {
            Err(AgentExecutorError::Cancelled) => {
                println!("Test: Operation correctly resulted in Cancelled error.");
            }
            // Depending on timing, it might also be ApprovalResponseChannelClosed if cancellation happens
            // exactly when the executor tries to read from the closed channel after its select loop breaks.
            Err(AgentExecutorError::ApprovalResponseChannelClosed) => {
                println!(
                    "Test: Operation correctly resulted in ApprovalResponseChannelClosed error due to cancellation timing."
                );
            }
            Ok(_) => panic!("Expected operation to fail due to cancellation, but it succeeded."),
            Err(e) => panic!(
                "Expected Cancelled or ApprovalResponseChannelClosed, got {:?}",
                e
            ),
        }
    }

    // --- Test Case: Tool Execution Error Handling ---
    #[tokio::test]
    #[ignore] // Requires real API calls
    async fn test_run_operation_tool_execution_error_real() {
        // --- Arrange ---
        let client = get_real_client();
        let executor = AgentExecutor::new(client.clone());
        let model = Model::Gpt4_1Nano20250414;
        let initial_messages = vec![text_message(
            MessageRole::User,
            "Use the 'error_prone_tool' tool to do something.",
        )];
        let tool_name = "error_prone_tool";
        let error_message = "This tool failed intentionally for testing";

        let available_tools = vec![Tool {
            name: tool_name.to_string(),
            description: "A tool that will produce an error".to_string(),
            input_schema: InputSchema {
                properties: json!({}).as_object().unwrap().clone(),
                required: vec![],
                schema_type: "object".to_string(),
            },
        }];
        let (event_tx, mut event_rx) = mpsc::channel(20);
        let token = CancellationToken::new();

        // Tool executor callback that always returns an error
        let tool_executor_callback = move |call: ToolCall, _token: CancellationToken| async move {
            // Verify it's our expected tool
            assert_eq!(call.name, tool_name);

            // Return a deliberate error
            Err(ToolError::Execution {
                tool_name: call.name.clone(),
                message: error_message.to_string(),
            })
        };

        // --- Act ---
        let executor_handle = tokio::spawn(async move {
            executor
                .run(
                    model,
                    initial_messages,
                    Some("Use tools when requested.".to_string()),
                    available_tools,
                    tool_executor_callback,
                    event_tx,
                    ApprovalMode::Automatic, // Using automatic mode for simplicity
                    token,
                )
                .await
        });

        // --- Assert Events ---
        let mut saw_executing_tool = false;
        let mut saw_tool_error_result = false;
        let mut saw_final_message = false;
        let mut received_tool_call_id = None;

        loop {
            tokio::select! {
                biased; // Prioritize timeout
                _ = tokio::time::sleep(Duration::from_secs(30)) => { // Generous timeout
                    panic!("Test timed out waiting for events or completion");
                }
                maybe_event = event_rx.recv() => {
                    match maybe_event {
                        Some(AgentEvent::AssistantMessageFinal(msg)) => {
                            // Store tool call ID if this is the first message with the tool call
                            if let MessageContent::StructuredContent { content } = &msg.content {
                                if let Some(coder::api::messages::ContentBlock::ToolUse { id, name, .. }) =
                                    content.0.iter().find(|b| matches!(b, coder::api::messages::ContentBlock::ToolUse{..})) {
                                    if name == tool_name {
                                        received_tool_call_id = Some(id.clone());
                                        println!("Test: Found tool call ID: {}", id);
                                    }
                                }
                            }

                            // Check if this is a final message after tool error
                            let text = msg.content.extract_text().to_lowercase();
                            if text.contains("error") || text.contains("fail") || text.contains("unable") {
                                saw_final_message = true;
                                println!("Test: Received final message acknowledging the error");
                            }
                        },
                        Some(AgentEvent::ExecutingTool{ tool_call_id: id, name }) => {
                            println!("Test: Received ExecutingTool");
                            assert_eq!(name, tool_name);
                            received_tool_call_id = Some(id.clone());
                            saw_executing_tool = true;
                        },
                        Some(AgentEvent::ToolResultReceived(result)) => {
                            if let Some(id) = &received_tool_call_id {
                                assert_eq!(&result.tool_call_id, id, "Tool result ID mismatch");
                            }
                            assert!(result.is_error, "Expected tool result to be an error");
                            assert!(result.output.contains(error_message),
                                "Error message doesn't contain expected text. Got: {}", result.output);
                            saw_tool_error_result = true;
                            println!("Test: Received error tool result: {}", result.output);
                        },
                        Some(AgentEvent::AssistantMessagePart(_)) => {}, // Ignore parts
                        None => break, // Channel closed
                        e => println!("Test: Got event: {:?}", e),
                    }
                },
            }

            // Exit when we've seen both the error result and the final message
            if saw_tool_error_result && saw_final_message {
                break;
            }
        }

        let final_result = executor_handle.await.unwrap();

        // --- Final Assertions ---
        assert!(
            final_result.is_ok(),
            "Operation should still complete successfully with a tool error"
        );
        assert!(saw_executing_tool, "Did not receive ExecutingTool event");
        assert!(
            saw_tool_error_result,
            "Did not receive error ToolResultReceived event"
        );
        assert!(
            saw_final_message,
            "Did not receive final message acknowledging the error"
        );

        let final_message = final_result.unwrap();
        let final_text = final_message.content.extract_text().to_lowercase();
        assert!(
            final_text.contains("error")
                || final_text.contains("fail")
                || final_text.contains("unable"),
            "Final message should acknowledge the tool error: {}",
            final_text
        );
    }

    // TODO: Add test for API error handling (requires modifying client or mocking)
}
