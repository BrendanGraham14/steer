// use anyhow::Result;
use coder::api::messages::{ContentBlock, MessageContent, StructuredContent};
use coder::api::{Client, Message, Model};
use coder::config::LlmConfig;
use dotenv::dotenv;
use std::env;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore]
async fn test_claude_api_basic() {
    dotenv().ok();

    // Create client
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    // Create a simple message
    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text {
            content: "What is 2+2?".to_string(),
        },
        id: None,
    }];

    // Call Claude API with specified model
    let response = client
        .complete(
            Model::Claude3_5Haiku20241022,
            messages,
            None,
            None,
            CancellationToken::new(),
        )
        .await;

    // Check if the response is successful
    assert!(response.is_ok(), "API call failed: {:?}", response.err());

    // Print the response content
    let response = response.unwrap();

    // Extract text from response
    let text = response.extract_text();
    println!("Claude response: {}", text);

    // Verify we got a reasonable response
    assert!(!text.is_empty(), "Response text should not be empty");
    assert!(text.contains("4"), "Response should contain the answer '4'");

    println!("Basic API test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_claude_api_with_tools() {
    // Load environment variables from .env file
    dotenv().ok();

    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    // Get all tools to send to Claude
    // Create tools using the ToolExecutor's to_api_tools method
    let tool_executor = coder::app::ToolExecutor::new();
    let tools = tool_executor.to_api_tools();

    // Create a message that will use a tool
    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text {
            content: "Please list the files in the current directory using the LS tool".to_string(),
        },
        id: None,
    }];

    let response = client
        .complete(
            Model::Claude3_5Haiku20241022,
            messages,
            None,
            Some(tools),
            CancellationToken::new(),
        )
        .await;

    // Debug output
    if response.is_err() {
        println!("API Error: {:?}", response.as_ref().err());
    }

    // Check if the response is successful
    assert!(response.is_ok(), "API call failed: {:?}", response.err());

    // Get the response
    let response = response.unwrap();

    // Check if the response contains a tool call
    println!("Has tool calls: {}", response.has_tool_calls());
    assert!(
        response.has_tool_calls(),
        "Response should contain tool calls"
    );

    // Extract and process tool calls
    let tool_calls = response.extract_tool_calls();
    assert!(!tool_calls.is_empty(), "Should have at least one tool call");
    println!("Tool calls: {:#?}", tool_calls);

    // Process the first tool call
    let first_tool_call = &tool_calls[0];
    println!("Tool call: {}", first_tool_call.name);
    println!(
        "Parameters: {}",
        serde_json::to_string_pretty(&first_tool_call.parameters).unwrap()
    );

    // Execute the tool using ToolExecutor with cancellation
    let tool_executor = coder::app::ToolExecutor::new();
    let result = tool_executor
        .execute_tool_with_cancellation(first_tool_call, tokio_util::sync::CancellationToken::new())
        .await;
    assert!(result.is_ok(), "Tool execution failed: {:?}", result.err());

    println!("Tool result: {}", result.unwrap());
    println!("Tools API test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_claude_api_with_tool_response() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let messages = vec![
        Message {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "Please list the files in the current directory using the LS tool"
                    .to_string(),
            },
            id: None,
        },
        Message {
            role: "assistant".to_string(),
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![ContentBlock::ToolUse {
                    id: "this-is-the-id".to_string(),
                    name: "ls".to_string(),
                    input: serde_json::Value::Null,
                }]),
            },
            id: None,
        },
        Message {
            role: "user".to_string(),
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "this-is-the-id".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "foo".to_string(),
                        }],
                        is_error: None,
                    },
                    ContentBlock::Text {
                        text: "list it again".to_string(),
                    },
                ]),
            },
            id: None,
        },
    ];

    let response = client
        .complete(
            Model::Claude3_5Haiku20241022,
            messages,
            None,
            None,
            CancellationToken::new(),
        )
        .await;

    assert!(response.is_ok(), "API call failed: {:?}", response.err());
}
