use coder::api::ApiError;
use coder::api::messages::{ContentBlock, MessageContent, MessageRole, StructuredContent};
use coder::api::tools::{InputSchema, Tool};
use coder::api::{Client, Message, Model};
use coder::config::LlmConfig;
use dotenv::dotenv;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore]
async fn test_api_basic() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let models_to_test = vec![
        Model::Claude3_5Haiku20241022,
        Model::Gpt4_1Nano20250414,
        Model::Gemini2_5FlashPreview0417,
        Model::ClaudeSonnet4_20250514,
    ];

    let mut tasks = Vec::new();

    // Create the simple message once
    let messages = vec![Message {
        role: MessageRole::User,
        content: MessageContent::Text {
            content: "What is 2+2?".to_string(),
        },
        id: None,
    }];

    for model in models_to_test {
        let client = client.clone(); // Clone Arc
        let messages = messages.clone(); // Clone messages
        let task = tokio::spawn(async move {
            println!("Testing basic API for model: {:?}", model);

            // Call API with specified model
            let response = client
                .complete(
                    model.clone(),
                    messages,
                    None,
                    None,
                    CancellationToken::new(), // Each task gets its own token
                )
                .await;

            // Check if the response is successful
            let response = response.map_err(|e| {
                eprintln!("API call failed for model {:?}: {:?}", model, e);
                e // Return the original ApiError
            })?; // Propagate error if response.is_err()

            // Extract text from response
            let text = response.extract_text();
            println!("{:?} response: {}", model, text);

            // Verify we got a reasonable response
            assert!(
                !text.is_empty(),
                "Response text should not be empty for model {:?}",
                model
            );
            // Allow variations like "4." or "four"
            assert!(
                text.contains("4") || text.to_lowercase().contains("four"),
                "Response for model {:?} should contain the answer '4'",
                model
            );

            println!("Basic API test for {:?} passed successfully!", model);
            Ok::<_, ApiError>(model) // Return model on success
        });
        tasks.push(task);
    }

    // Wait for all tasks to complete
    let results = futures::future::join_all(tasks).await;

    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(Ok(model)) => {
                println!("Task for {:?} finished successfully.", model);
            }
            Ok(Err(e)) => {
                // Task completed, but API call failed (already logged in task)
                let msg = format!("API call failed within task: {:?}", e);
                failures.push(msg);
            }
            Err(e) => {
                // Task panicked (includes assertion failures)
                let msg = format!("Task panicked: {:?}", e);
                eprintln!("{}", msg); // Log the error immediately
                failures.push(msg);
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "One or more basic API test tasks failed:\n{}",
            failures.join("\n")
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_api_with_tools() {
    // Load environment variables from .env file
    dotenv().ok();

    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config); // Arc<Client>

    let models_to_test = vec![
        Model::Claude3_5Haiku20241022,
        Model::Gpt4_1Nano20250414,
        Model::Gemini2_5FlashPreview0417,
    ];

    let mut tasks = Vec::new();

    // Get current directory once
    let pwd = std::env::current_dir().unwrap();
    // Get tools once
    let mut backend_registry = coder::tools::BackendRegistry::new();
    backend_registry.register(
        "local".to_string(),
        std::sync::Arc::new(coder::tools::LocalBackend::standard()),
    );
    let tool_executor_template =
        coder::app::ToolExecutor::new(std::sync::Arc::new(backend_registry));
    let tools = tool_executor_template.to_api_tools(); // Clone this Vec<Tool>

    for model in models_to_test {
        let client = client.clone(); // Clone Arc
        let tools = tools.clone(); // Clone tools definition
        let pwd_display = pwd.display().to_string(); // Clone path string

        let task = tokio::spawn(async move {
            println!("Testing API with tools for model: {:?}", model);

            // Create a message that will use a tool
            let messages = vec![Message {
                role: MessageRole::User,
                content: MessageContent::Text {
                    content: format!(
                        "Please list the files in {} using the LS tool",
                        pwd_display // Use cloned path string
                    ),
                },
                id: None,
            }];

            let response = client
                .complete(
                    model.clone(),
                    messages,
                    None,
                    Some(tools),              // Use cloned tools
                    CancellationToken::new(), // Each task gets its own token
                )
                .await;

            // Debug output and check if the response is successful
            let response = response.map_err(|e| {
                eprintln!("API Error for model {:?}: {:?}", model, e);
                e
            })?; // Propagate error

            // Check if the response contains a tool call
            println!("{:?} Has tool calls: {}", model, response.has_tool_calls());
            assert!(
                response.has_tool_calls(),
                "Response for model {:?} should contain tool calls",
                model
            );

            // Extract and process tool calls
            let tool_calls = response.extract_tool_calls();
            assert!(
                !tool_calls.is_empty(),
                "Should have at least one tool call for model {:?}",
                model
            );
            println!("{:?} Tool calls: {:#?}", model, tool_calls);

            // Process the first tool call
            // Ensure the correct tool is being called (ls)
            let first_tool_call = tool_calls
                .iter()
                .find(|tc| tc.name == "ls")
                .expect(&format!("Expected 'ls' tool call for model {:?}", model));

            println!("{:?} Tool call: {}", model, first_tool_call.name);
            // Optional: Pretty print parameters only if needed for debugging
            // println!(
            //     "{:?} Parameters: {}",
            //     model,
            //     serde_json::to_string_pretty(&first_tool_call.parameters).unwrap()
            // );

            // Execute the tool using ToolExecutor with cancellation
            let mut backend_registry = coder::tools::BackendRegistry::new();
            backend_registry.register(
                "local".to_string(),
                std::sync::Arc::new(coder::tools::LocalBackend::standard()),
            );
            let tool_executor =
                coder::app::ToolExecutor::new(std::sync::Arc::new(backend_registry));
            let result = tool_executor
                .execute_tool_with_cancellation(
                    first_tool_call,
                    tokio_util::sync::CancellationToken::new(), // Use a separate token for tool execution
                )
                .await;

            // Assert tool execution success within the task
            assert!(
                result.is_ok(),
                "Tool execution failed for model {:?}: {:?}",
                model,
                result.err() // Use .err() for assertion message
            );

            println!("{:?} Tool result: {}", model, result.unwrap()); // Unwrap after assertion
            println!("Tools API test for {:?} passed successfully!", model);

            Ok::<_, ApiError>(model) // Return model on success
        });
        tasks.push(task);
    }

    // Wait for all tasks to complete
    let results = futures::future::join_all(tasks).await;

    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(Ok(model)) => {
                println!("Task for {:?} finished successfully.", model);
            }
            Ok(Err(e)) => {
                // Task completed, but API call failed (already logged in task)
                let msg = format!("API call failed within task: {:?}", e);
                failures.push(msg);
            }
            Err(e) => {
                // Task panicked (includes assertion failures)
                let msg = format!("Task panicked: {:?}", e);
                eprintln!("{}", msg); // Log the error immediately
                failures.push(msg);
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "One or more API with tools test tasks failed:\n{}",
            failures.join("\n")
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_api_with_tool_response() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let models_to_test = vec![
        Model::Claude3_5Haiku20241022,
        Model::Gpt4_1Nano20250414,
        Model::Gemini2_5FlashPreview0417,
    ];
    let mut tasks = Vec::new();

    for model in models_to_test {
        let client = client.clone(); // Clone Arc for concurrent use
        let task = tokio::spawn(async move {
            println!("Testing API with tool response for model: {:?}", model);

            // Construct messages specific to this model's task
            let tool_use_id = format!("tool-use-id-{:?}", model); // Unique ID per model test
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
                        content: StructuredContent(vec![
                            ContentBlock::ToolUse {
                                id: tool_use_id.clone(),
                                name: "ls".to_string(),
                                input: serde_json::json!({ "path": "." }),
                            }
                        ]),
                    },
                    id: None,
                },
                Message {
                    role: MessageRole::User,
                    content: MessageContent::StructuredContent {
                        content: StructuredContent(vec![
                            ContentBlock::ToolResult {
                                tool_use_id: tool_use_id.clone(), // Match the tool use ID
                                content: vec![ContentBlock::Text {
                                    text: "foo.txt, bar.rs".to_string(), // Example result
                                }],
                                is_error: None,
                            },
                            ContentBlock::Text {
                                text: "What was the name of the rust file?".to_string(),
                            },
                        ]),
                    },
                    id: None,
                },
            ];

            let response = client
                .complete(
                    model.clone(),
                    messages,
                    None,
                    None, // No tools needed here as we are providing the tool result
                    CancellationToken::new(), // Each task gets its own token
                )
                .await;

            // Check response and propagate error
            let response = response.map_err(|e| {
                eprintln!("API call failed for model {:?}: {:?}", model, e);
                e
            })?;

            // Add assertions for the final response
            let final_text = response.extract_text();
            assert!(
                !final_text.is_empty(),
                "Final response text should not be empty for model {:?}",
                model
            );
            assert!(
                final_text.to_lowercase().contains("bar.rs"), // Example assertion: Check if the model focused on the requested file type
                "Final response for model {:?} should mention 'bar.rs', got: '{}'",
                model,
                final_text
            );

            println!("Tool Response test for {:?} passed successfully!", model);
            Ok::<_, ApiError>(model) // Return model on success
        });
        tasks.push(task);
    }

    // Wait for all tasks to complete
    let results = futures::future::join_all(tasks).await;

    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(Ok(model)) => {
                println!("Task for {:?} finished successfully.", model);
            }
            Ok(Err(e)) => {
                // Task completed, but API call failed (already logged in task)
                let msg = format!("API call failed within task: {:?}", e);
                failures.push(msg);
            }
            Err(e) => {
                // Task panicked (includes assertion failures)
                let msg = format!("Task panicked: {:?}", e);
                eprintln!("{}", msg); // Log the error immediately
                failures.push(msg);
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "One or more API tool response test tasks failed:\n{}",
            failures.join("\n")
        );
    }
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
    let mut backend_registry = coder::tools::BackendRegistry::new();
    backend_registry.register(
        "local".to_string(),
        std::sync::Arc::new(coder::tools::LocalBackend::standard()),
    );
    let tool_executor = coder::app::ToolExecutor::new(std::sync::Arc::new(backend_registry));
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

#[tokio::test]
#[ignore]
async fn test_api_with_cancelled_tool_execution() {
    dotenv().ok();
    let config = LlmConfig::from_env().unwrap();
    let client = Client::new(&config);

    let models_to_test = vec![
        Model::Claude3_5Haiku20241022,
        Model::Gpt4_1Nano20250414,
        Model::Gemini2_5FlashPreview0417,
    ];
    let mut tasks = Vec::new();

    for model in models_to_test {
        let client = client.clone();
        let task = tokio::spawn(async move {
            println!("Testing cancelled tool execution for model: {:?}", model);

            // Create a unique tool call ID for this test
            let tool_call_id = format!("cancelled_tool_{:?}", model);

            // Simulate a conversation where a tool was called but then cancelled
            let messages = vec![
                Message {
                    role: MessageRole::User,
                    content: MessageContent::Text {
                        content: "Please list the files in the current directory".to_string(),
                    },
                    id: None,
                },
                // Assistant requests a tool call
                Message {
                    role: MessageRole::Assistant,
                    content: MessageContent::StructuredContent {
                        content: StructuredContent(vec![
                            ContentBlock::ToolUse {
                                id: tool_call_id.clone(),
                                name: "ls".to_string(),
                                input: serde_json::json!({ "path": "." }),
                            }
                        ]),
                    },
                    id: None,
                },
                // Tool execution was cancelled - this is what inject_cancelled_tool_results would add
                Message {
                    role: MessageRole::Tool,
                    content: MessageContent::StructuredContent {
                        content: StructuredContent(vec![
                            ContentBlock::ToolResult {
                                tool_use_id: tool_call_id.clone(),
                                content: vec![ContentBlock::Text {
                                    text: "Tool execution was cancelled by user before completion.".to_string(),
                                }],
                                is_error: None, // Not marked as error, just cancelled
                            }
                        ]),
                    },
                    id: None,
                },
                // User continues the conversation
                Message {
                    role: MessageRole::User,
                    content: MessageContent::Text {
                        content: "No problem, can you tell me about Rust instead?".to_string(),
                    },
                    id: None,
                },
            ];

            // Call the API - it should handle the cancelled tool result gracefully
            let response = client
                .complete(
                    model.clone(),
                    messages,
                    None,
                    None, // No tools needed as we're testing message handling
                    CancellationToken::new(),
                )
                .await;

            // Check response
            let response = response.map_err(|e| {
                eprintln!("API call failed for model {:?} with cancelled tool: {:?}", model, e);
                e
            })?;

            // Extract and verify response
            let response_text = response.extract_text();
            assert!(
                !response_text.is_empty(),
                "Response should not be empty for model {:?} after cancelled tool",
                model
            );

            // The model should acknowledge the cancellation and answer about Rust
            assert!(
                response_text.to_lowercase().contains("rust") ||
                response_text.to_lowercase().contains("programming") ||
                response_text.to_lowercase().contains("language"),
                "Response for model {:?} should address the Rust question, got: '{}'",
                model,
                response_text
            );

            println!("Cancelled tool execution test for {:?} passed successfully!", model);
            Ok::<_, ApiError>(model)
        });
        tasks.push(task);
    }

    // Wait for all tasks to complete
    let results = futures::future::join_all(tasks).await;

    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(Ok(model)) => {
                println!("Task for {:?} finished successfully.", model);
            }
            Ok(Err(e)) => {
                let msg = format!("API call failed within task: {:?}", e);
                failures.push(msg);
            }
            Err(e) => {
                let msg = format!("Task panicked: {:?}", e);
                eprintln!("{}", msg);
                failures.push(msg);
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "One or more cancelled tool execution test tasks failed:\n{}",
            failures.join("\n")
        );
    }
}
