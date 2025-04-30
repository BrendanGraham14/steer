use coder::api::messages::{ContentBlock, MessageContent, MessageRole, StructuredContent};
use coder::api::tools::{InputSchema, Tool};
use coder::api::{Client, Message, Model};
use coder::config::LlmConfig;
use dotenv::dotenv;
use serde_json::json;
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
        role: MessageRole::User,
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
    let pwd = std::env::current_dir().unwrap();
    let messages = vec![Message {
        role: MessageRole::User,
        content: MessageContent::Text {
            content: format!(
                "Please list the files in {} using the LS tool",
                pwd.display()
            ),
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
            role: MessageRole::User,
            content: MessageContent::Text {
                content: "Please list the files in the current directory using the LS tool"
                    .to_string(),
            },
            id: None,
        },
        Message {
            role: MessageRole::Assistant,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![ContentBlock::ToolUse {
                    id: "this-is-the-id".to_string(),
                    name: "ls".to_string(),
                    input: serde_json::json!({ "path": "." }),
                }]),
            },
            id: None,
        },
        Message {
            role: MessageRole::User,
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

#[tokio::test]
#[ignore]
async fn test_openai_nano_basic() {
    dotenv().ok();

    // Create client
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    // Create a simple message
    let messages = vec![Message {
        role: MessageRole::User,
        content: MessageContent::Text {
            content: "What is 2+2?".to_string(),
        },
        id: None,
    }];

    // Call OpenAI API with specified model
    let response = client
        .complete(
            Model::Gpt4_1Nano20250414, // Use OpenAI Nano model
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
    println!("OpenAI Nano response: {}", text);

    // Verify we got a reasonable response
    assert!(!text.is_empty(), "Response text should not be empty");
    assert!(text.contains("4"), "Response should contain the answer '4'");

    println!("Basic OpenAI Nano API test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_openai_nano_with_tools() {
    // Load environment variables from .env file
    dotenv().ok();

    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    // Get all tools to send to the API
    let tool_executor = coder::app::ToolExecutor::new();
    let tools = tool_executor.to_api_tools();

    // Create a message that will use a tool
    let pwd = std::env::current_dir().unwrap();
    let messages = vec![Message {
        role: MessageRole::User,
        content: MessageContent::Text {
            content: format!(
                "Please list the files in {} using the LS tool",
                pwd.display()
            ),
        },
        id: None,
    }];

    let response = client
        .complete(
            Model::Gpt4_1Nano20250414, // Use OpenAI Nano model
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
    println!("OpenAI Nano Tools API test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_openai_nano_with_tool_response() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let messages = vec![
        Message {
            role: MessageRole::User,
            content: MessageContent::Text {
                content: "Please list the files in the current directory using the LS tool"
                    .to_string(),
            },
            id: None,
        },
        Message {
            role: MessageRole::Assistant,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![ContentBlock::ToolUse {
                    id: "this-is-the-id".to_string(),
                    name: "ls".to_string(),
                    input: serde_json::json!({ "path": "." }),
                }]),
            },
            id: None,
        },
        Message {
            role: MessageRole::User,
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
            Model::Gpt4_1Nano20250414, // Use OpenAI Nano model
            messages,
            None,
            None, // No tools needed here as we are providing the tool result
            CancellationToken::new(),
        )
        .await;

    assert!(response.is_ok(), "API call failed: {:?}", response.err());
    println!("OpenAI Nano Tool Response test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_gemini_api_basic() {
    dotenv().ok();

    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let messages = vec![Message {
        role: MessageRole::User,
        content: MessageContent::Text {
            content: "What is 2+2?".to_string(),
        },
        id: None,
    }];

    let response = client
        .complete(
            Model::Gemini2_5FlashPreview0417,
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

    let text = response.extract_text();
    println!("Gemini response: {}", text);

    assert!(!text.is_empty(), "Response text should not be empty");
    assert!(text.contains("4"), "Response should contain the answer '4'");

    println!("Basic Gemini API test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_gemini_api_with_tools() {
    dotenv().ok();

    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let tool_executor = coder::app::ToolExecutor::new();
    let tools = tool_executor.to_api_tools();

    let pwd = std::env::current_dir().unwrap();
    let messages = vec![Message {
        role: MessageRole::User,
        content: MessageContent::Text {
            content: format!(
                "Please list the files in {} using the LS tool",
                pwd.display()
            ),
        },
        id: None,
    }];

    let response = client
        .complete(
            Model::Gemini2_5FlashPreview0417, // Use Gemini model
            messages,
            None,
            Some(tools),
            CancellationToken::new(),
        )
        .await;

    if response.is_err() {
        println!("API Error: {:?}", response.as_ref().err());
    }

    assert!(response.is_ok(), "API call failed: {:?}", response.err());

    let response = response.unwrap();

    println!("Has tool calls: {}", response.has_tool_calls());
    assert!(
        response.has_tool_calls(),
        "Response should contain tool calls"
    );

    let tool_calls = response.extract_tool_calls();
    assert!(!tool_calls.is_empty(), "Should have at least one tool call");
    println!("Tool calls: {:#?}", tool_calls);

    let first_tool_call = &tool_calls[0];
    println!("Tool call: {}", first_tool_call.name);
    println!(
        "Parameters: {}",
        serde_json::to_string_pretty(&first_tool_call.parameters).unwrap()
    );

    let tool_executor = coder::app::ToolExecutor::new();
    let result = tool_executor
        .execute_tool_with_cancellation(first_tool_call, tokio_util::sync::CancellationToken::new())
        .await;
    assert!(result.is_ok(), "Tool execution failed: {:?}", result.err());

    println!("Tool result: {}", result.unwrap());
    println!("Gemini Tools API test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_gemini_api_with_tool_response() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let messages = vec![
        Message {
            role: MessageRole::User,
            content: MessageContent::Text {
                content: "Please list the files in the current directory using the LS tool"
                    .to_string(),
            },
            id: None,
        },
        Message {
            role: MessageRole::Assistant,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![ContentBlock::ToolUse {
                    id: "this-is-the-id".to_string(),
                    name: "ls".to_string(),
                    input: serde_json::json!({ "path": "." }),
                }]),
            },
            id: None,
        },
        Message {
            role: MessageRole::User,
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
            Model::Gemini2_5FlashPreview0417, // Use Gemini model
            messages,
            None,
            None, // No tools needed here as we are providing the tool result
            CancellationToken::new(),
        )
        .await;

    assert!(response.is_ok(), "API call failed: {:?}", response.err());
    println!("Gemini Tool Response test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_gemini_system_instructions() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let messages = vec![Message {
        role: MessageRole::User,
        content: MessageContent::Text {
            content: "What's your name?".to_string(),
        },
        id: None,
    }];

    let system =
        Some("Your name is GeminiHelper. Always introduce yourself as GeminiHelper.".to_string());

    let response = client
        .complete(
            Model::Gemini2_5FlashPreview0417, // Use Gemini model
            messages,
            system,
            None,
            CancellationToken::new(),
        )
        .await;

    assert!(response.is_ok(), "API call failed: {:?}", response.err());

    let response = response.unwrap();

    let text = response.extract_text();
    println!("Gemini response: {}", text);

    assert!(!text.is_empty(), "Response text should not be empty");
    assert!(
        text.contains("GeminiHelper"),
        "Response should contain the name 'GeminiHelper'"
    );

    println!("Gemini system instructions test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_gemini_api_tool_result_error() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let messages = vec![
        Message {
            role: MessageRole::User,
            content: MessageContent::Text {
                content: "Please list the files in the current directory using the LS tool"
                    .to_string(),
            },
            id: None,
        },
        Message {
            role: MessageRole::Assistant,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![ContentBlock::ToolUse {
                    id: "tool-use-id-error".to_string(),
                    name: "ls".to_string(),
                    input: serde_json::json!({ "path": "." }),
                }]),
            },
            id: None,
        },
        Message {
            role: MessageRole::User,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "tool-use-id-error".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "Error executing command".to_string(),
                        }],
                        // Mark this result as an error
                        is_error: Some(true),
                    },
                    ContentBlock::Text {
                        text: "Okay, thank you.".to_string(), // Provide some follow-up text
                    },
                ]),
            },
            id: None,
        },
    ];

    let response = client
        .complete(
            Model::Gemini2_5FlashPreview0417, // Use Gemini model
            messages,
            None,
            None, // No tools needed here as we are providing the tool result
            CancellationToken::new(),
        )
        .await;

    assert!(
        response.is_ok(),
        "API call failed when sending tool result with error: {:?}",
        response.err()
    );
    println!("Gemini Tool Result Error test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_gemini_api_complex_tool_schema() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    // Define a tool with a complex schema
    let complex_tool = Tool {
        name: "complex_operation".to_string(),
        description: "Performs a complex operation with nested parameters.".to_string(),
        input_schema: InputSchema {
            schema_type: "object".to_string(),
            properties: serde_json::map::Map::from_iter(vec![
                (
                    "config".to_string(),
                    json!({
                        "type": "object",
                        "properties": {
                            "retries": {"type": "integer", "description": "Number of retries"},
                            "enabled": {"type": "boolean", "description": "Whether the feature is enabled"}
                        },
                        "required": ["retries", "enabled"]
                    }),
                ),
                (
                    "items".to_string(),
                    json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "List of items to process"
                    }),
                ),
                (
                    "optional_param".to_string(),
                    json!({
                        "type": ["string", "null"], // Test nullable type
                        "description": "An optional parameter"
                    }),
                ),
            ]),
            required: vec!["config".to_string(), "items".to_string()],
        },
    };

    let messages = vec![Message {
        role: MessageRole::User,
        content: MessageContent::Text {
            content: "What is the weather today?".to_string(), // A simple message, doesn't need to invoke the tool
        },
        id: None,
    }];

    let response = client
        .complete(
            Model::Gemini2_5FlashPreview0417,
            messages,
            None,
            Some(vec![complex_tool]), // Send the complex tool definition
            CancellationToken::new(),
        )
        .await;

    assert!(
        response.is_ok(),
        "API call failed when sending complex tool schema: {:?}",
        response.err()
    );

    // We don't expect a tool call here, just that the API accepted the schema
    let response_data = response.unwrap();
    assert!(
        !response_data.has_tool_calls(),
        "Should not have made a tool call for a simple weather query"
    );

    println!("Gemini Complex Tool Schema test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_gemini_api_tool_result_json() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    // Define the JSON string to be used as the tool result content
    let json_result_string =
        serde_json::json!({ "status": "success", "files": ["file1.txt", "file2.log"] }).to_string();

    let messages = vec![
        Message {
            role: MessageRole::User,
            content: MessageContent::Text {
                content: "Run the file listing tool.".to_string(),
            },
            id: None,
        },
        Message {
            role: MessageRole::Assistant,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![ContentBlock::ToolUse {
                    id: "tool-use-id-json".to_string(),
                    name: "list_files".to_string(),
                    input: serde_json::json!({}), // Empty input for simplicity
                }]),
            },
            id: None,
        },
        Message {
            role: MessageRole::User,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "tool-use-id-json".to_string(),
                        content: vec![ContentBlock::Text {
                            // Use the JSON string as the text content
                            text: json_result_string,
                        }],
                        is_error: None,
                    },
                    ContentBlock::Text {
                        text: "Thanks for the list.".to_string(),
                    },
                ]),
            },
            id: None,
        },
    ];

    let response = client
        .complete(
            Model::Gemini2_5FlashPreview0417, // Use Gemini model
            messages,
            None,
            None, // No tools needed here as we are providing the tool result
            CancellationToken::new(),
        )
        .await;

    assert!(
        response.is_ok(),
        "API call failed when sending tool result with JSON content: {:?}",
        response.err()
    );
    println!("Gemini Tool Result JSON Content test passed successfully!");
}

#[tokio::test]
#[ignore]
async fn test_gemini_api_with_multiple_tool_responses() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let messages = vec![
        Message {
            role: MessageRole::User,
            content: MessageContent::Text {
                content: "Please list files in '.' and check the weather in 'SF'".to_string(),
            },
            id: None,
        },
        // Assistant makes two tool calls
        Message {
            role: MessageRole::Assistant,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![
                    ContentBlock::ToolUse {
                        id: "tool-use-id-1".to_string(),
                        name: "ls".to_string(),
                        input: serde_json::json!({ "path": "." }),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-use-id-2".to_string(),
                        name: "get_weather".to_string(),
                        input: serde_json::json!({ "location": "SF" }),
                    },
                ]),
            },
            id: None,
        },
        // User provides results for both tool calls in one message
        Message {
            role: MessageRole::User,
            content: MessageContent::StructuredContent {
                content: StructuredContent(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "tool-use-id-1".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "file1.rs, file2.toml".to_string(),
                        }],
                        is_error: None,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "tool-use-id-2".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "Sunny, 20C".to_string(),
                        }],
                        is_error: None,
                    },
                    // Optional: Add text after results
                    ContentBlock::Text {
                        text: "Got it, thanks!".to_string(),
                    },
                ]),
            },
            id: None,
        },
    ];

    // Define the 'get_weather' tool for the API call, 'ls' is usually predefined
    let weather_tool = Tool {
        name: "get_weather".to_string(),
        description: "Gets the weather for a location".to_string(),
        input_schema: InputSchema {
            schema_type: "object".to_string(),
            properties: serde_json::map::Map::from_iter(vec![(
                "location".to_string(),
                json!({"type": "string", "description": "The location to get weather for"}),
            )]),
            required: vec!["location".to_string()],
        },
    };
    // Assuming ToolExecutor provides 'ls' or similar standard tools
    let tool_executor = coder::app::ToolExecutor::new();
    let mut tools = tool_executor.to_api_tools();
    tools.push(weather_tool);


    let response = client
        .complete(
            Model::Gemini2_5FlashPreview0417, // Use Gemini model
            messages,
            None,
            Some(tools), // Provide tools including the dummy weather tool
            CancellationToken::new(),
        )
        .await;

    assert!(
        response.is_ok(),
        "API call failed when sending multiple tool results: {:?}",
        response.err()
    );
    println!("Gemini Multiple Tool Responses test passed successfully!");
}

