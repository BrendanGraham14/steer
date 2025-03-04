use anyhow::Result;
use claude_code_rs::api::{Client, Message, Tool, ToolCall};
use dotenv::dotenv;
use std::env;

#[tokio::test]
#[ignore]
async fn test_claude_api_basic() {
    // Load environment variables from .env file
    dotenv().ok();

    // Get API key from environment
    let api_key = match env::var("CLAUDE_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            println!("CLAUDE_API_KEY not found in environment. Skipping test.");
            return;
        }
    };

    // Create client
    let client = Client::new(&api_key);

    // Create a simple message
    let messages = vec![Message {
        role: "user".to_string(),
        content: "What is 2+2?".to_string(),
    }];

    // Call Claude API
    let response = client.complete(messages, None, None).await;

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

    // Get API key from environment
    let api_key = match env::var("CLAUDE_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            println!("CLAUDE_API_KEY not found in environment. Skipping test.");
            return;
        }
    };

    // Create client
    let client = Client::new(&api_key);

    // Get all tools to send to Claude
    let tools = Tool::all();

    // Create a message that will use a tool
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Please list the files in the current directory using the LS tool".to_string(),
    }];

    // Call Claude API with tools
    let response = client.complete(messages, None, Some(tools)).await;

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

    // Execute the tool manually
    let result = execute_tool(first_tool_call).await;
    assert!(result.is_ok(), "Tool execution failed: {:?}", result.err());

    println!("Tool result: {}", result.unwrap());
    println!("Tools API test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_streaming_response() -> Result<()> {
    // Load environment variables from .env file
    dotenv().ok();

    // Get API key from environment
    let api_key = match env::var("CLAUDE_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            println!("CLAUDE_API_KEY not found in environment. Skipping test.");
            return Ok(());
        }
    };

    // Create client
    let client = Client::new(&api_key);

    // Create a simple message
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Write a short poem about Rust programming".to_string(),
    }];

    // Get streaming response
    let mut stream = client.complete_streaming(messages, None, None);

    // Process the stream
    use futures_util::StreamExt;
    let mut text = String::new();

    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(content) => {
                println!("Received chunk: {}", content);
                text.push_str(&content);
            }
            Err(e) => {
                println!("Stream error: {}", e);
                assert!(false, "Stream error: {}", e);
            }
        }
    }

    // Verify we got a reasonable response
    assert!(!text.is_empty(), "Response text should not be empty");
    assert!(text.contains("Rust"), "Response should mention Rust");

    println!("Final response: {}", text);
    println!("Streaming API test passed successfully!");

    Ok(())
}

// Helper function to execute a tool call
async fn execute_tool(tool_call: &ToolCall) -> Result<String> {
    // Use the top-level execute_tool function from the tools module
    claude_code_rs::tools::execute_tool(&tool_call.name, &tool_call.parameters).await
}
