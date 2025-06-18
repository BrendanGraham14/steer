#[cfg(test)]
mod tests {
    use crate::api::Model;
    use crate::app::validation::ValidatorRegistry;
    use crate::app::{
        App, AppConfig, ToolExecutor,
        conversation::{Message, ToolResult, AssistantContent},
    };
    use crate::config::LlmConfig;
    use crate::tools::{BackendRegistry, LocalBackend};
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tools::ToolCall;

    fn create_test_llm_config() -> LlmConfig {
        LlmConfig {
            anthropic_api_key: Some("test_key".to_string()),
            openai_api_key: None,
            gemini_api_key: None,
        }
    }

    fn create_test_tool_executor() -> Arc<ToolExecutor> {
        let mut backend_registry = BackendRegistry::new();
        backend_registry.register("local".to_string(), Arc::new(LocalBackend::full()));

        Arc::new(ToolExecutor::with_components(
            None, // No workspace for tests
            Arc::new(backend_registry),
            Arc::new(ValidatorRegistry::new()),
        ))
    }

    /// Test that incomplete tool calls are detected and cancelled tool results are injected
    #[tokio::test]
    async fn test_inject_cancelled_tool_results() {
        // Create a minimal app configuration
        let llm_config = create_test_llm_config();
        let app_config = AppConfig { llm_config };
        let (event_tx, _event_rx) = mpsc::channel(100);
        let initial_model = Model::Claude3_7Sonnet20250219;

        let mut app = App::new(
            app_config,
            event_tx,
            initial_model,
            create_test_tool_executor(),
        )
        .unwrap();

        // Manually add an assistant message with tool calls
        let tool_call = ToolCall {
            id: "test_tool_123".to_string(),
            name: "test_tool".to_string(),
            parameters: serde_json::json!({"param": "value"}),
        };

        let assistant_msg = Message::Assistant {
            content: vec![
                AssistantContent::Text { text: "I'll use a tool to help with that.".to_string() },
                AssistantContent::ToolCall { tool_call: tool_call.clone() },
            ],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("assistant", Message::current_timestamp()),
        };

        app.add_message(assistant_msg).await;

        // Verify that we have an incomplete tool call
        {
            let conversation_guard = app.conversation.lock().await;
            let incomplete_calls = app.find_incomplete_tool_calls(&conversation_guard);
            assert_eq!(incomplete_calls.len(), 1);
            assert_eq!(incomplete_calls[0].id, "test_tool_123");
            assert_eq!(incomplete_calls[0].name, "test_tool");
        }

        // Now inject cancelled tool results
        app.inject_cancelled_tool_results().await;

        // Verify that a cancelled tool result was added
        {
            let conversation_guard = app.conversation.lock().await;
            let messages = &conversation_guard.messages;

            // Should have 2 messages: the assistant message and the tool result
            assert_eq!(messages.len(), 2);

            // Check the tool result message
            let tool_result_msg = &messages[1];
            if let Message::Tool { tool_use_id, result, .. } = tool_result_msg {
                assert_eq!(tool_use_id, "test_tool_123");
                match result {
                    ToolResult::Success { output } => {
                        assert!(output.contains("cancelled"));
                    }
                    ToolResult::Error { error } => {
                        assert!(error.contains("cancelled"));
                    }
                }
            } else {
                panic!("Expected Tool message");
            }

            // Verify no more incomplete tool calls
            let incomplete_calls = app.find_incomplete_tool_calls(&conversation_guard);
            assert_eq!(incomplete_calls.len(), 0);
        }
    }

    /// Test that tool calls with existing results are not considered incomplete
    #[tokio::test]
    async fn test_complete_tool_calls_not_affected() {
        // Create a minimal app configuration
        let llm_config = create_test_llm_config();
        let app_config = AppConfig { llm_config };
        let (event_tx, _event_rx) = mpsc::channel(100);
        let initial_model = Model::Claude3_7Sonnet20250219;

        let mut app = App::new(
            app_config,
            event_tx,
            initial_model,
            create_test_tool_executor(),
        )
        .unwrap();

        // Add an assistant message with tool calls
        let tool_call = ToolCall {
            id: "complete_tool_456".to_string(),
            name: "complete_tool".to_string(),
            parameters: serde_json::json!({"param": "value"}),
        };

        let assistant_msg = Message::Assistant {
            content: vec![AssistantContent::ToolCall { tool_call: tool_call.clone() }],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("assistant", Message::current_timestamp()),
        };

        app.add_message(assistant_msg).await;

        // Add a tool result for this tool call
        let tool_result_msg = Message::Tool {
            tool_use_id: "complete_tool_456".to_string(),
            result: ToolResult::Success { output: "Tool completed successfully".to_string() },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("tool", Message::current_timestamp()),
        };

        app.add_message(tool_result_msg).await;

        // Verify that there are no incomplete tool calls
        {
            let conversation_guard = app.conversation.lock().await;
            let incomplete_calls = app.find_incomplete_tool_calls(&conversation_guard);
            assert_eq!(incomplete_calls.len(), 0);
        }

        // Inject cancelled tool results (should be a no-op)
        app.inject_cancelled_tool_results().await;

        // Verify that no additional messages were added
        {
            let conversation_guard = app.conversation.lock().await;
            let messages = &conversation_guard.messages;
            assert_eq!(messages.len(), 2); // Still just the assistant message and tool result
        }
    }

    /// Test multiple incomplete tool calls
    #[tokio::test]
    async fn test_multiple_incomplete_tool_calls() {
        // Create a minimal app configuration
        let llm_config = create_test_llm_config();
        let app_config = AppConfig { llm_config };
        let (event_tx, _event_rx) = mpsc::channel(100);
        let initial_model = Model::Claude3_7Sonnet20250219;

        let mut app = App::new(
            app_config,
            event_tx,
            initial_model,
            create_test_tool_executor(),
        )
        .unwrap();

        // Add an assistant message with multiple tool calls
        let tool_call_1 = ToolCall {
            id: "tool_1".to_string(),
            name: "bash".to_string(),
            parameters: serde_json::json!({"command": "ls"}),
        };

        let tool_call_2 = ToolCall {
            id: "tool_2".to_string(),
            name: "read_file".to_string(),
            parameters: serde_json::json!({"path": "/some/file"}),
        };

        let assistant_msg = Message::Assistant {
            content: vec![
                AssistantContent::Text { text: "I'll use multiple tools.".to_string() },
                AssistantContent::ToolCall { tool_call: tool_call_1.clone() },
                AssistantContent::ToolCall { tool_call: tool_call_2.clone() },
            ],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("assistant", Message::current_timestamp()),
        };

        app.add_message(assistant_msg).await;

        // Inject cancelled tool results
        app.inject_cancelled_tool_results().await;

        // Verify that cancelled tool results were added for both tools
        {
            let conversation_guard = app.conversation.lock().await;
            let messages = &conversation_guard.messages;

            // Should have 3 messages: assistant message + 2 tool result messages
            assert_eq!(messages.len(), 3);

            // Check that we have tool results for both tools
            let mut found_tool_1 = false;
            let mut found_tool_2 = false;

            for message in &messages[1..] {
                if let Message::Tool { tool_use_id, result, .. } = message {
                    if tool_use_id == "tool_1" {
                        found_tool_1 = true;
                        match result {
                            ToolResult::Success { output } => {
                                assert!(output.contains("cancelled"));
                            }
                            ToolResult::Error { error } => {
                                assert!(error.contains("cancelled"));
                            }
                        }
                    } else if tool_use_id == "tool_2" {
                        found_tool_2 = true;
                        match result {
                            ToolResult::Success { output } => {
                                assert!(output.contains("cancelled"));
                            }
                            ToolResult::Error { error } => {
                                assert!(error.contains("cancelled"));
                            }
                        }
                    }
                } else {
                    panic!("Expected Tool message");
                }
            }

            assert!(
                found_tool_1,
                "Should have found cancelled result for tool_1"
            );
            assert!(
                found_tool_2,
                "Should have found cancelled result for tool_2"
            );

            // Verify no more incomplete tool calls
            let incomplete_calls = app.find_incomplete_tool_calls(&conversation_guard);
            assert_eq!(incomplete_calls.len(), 0);
        }
    }
}
