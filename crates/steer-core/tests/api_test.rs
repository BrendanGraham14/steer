use dotenvy::dotenv;
use futures::StreamExt;
use rstest::rstest;
use steer_core::api::{ApiError, Client, Provider, StreamChunk};
use steer_core::app::SystemContext;
use steer_core::app::conversation::{AssistantContent, Message, MessageData, UserContent};
use steer_core::config::model::{ModelId, builtin};

use serde_json::json;
use steer_core::test_utils;
use steer_core::tools::ToolRegistry;
use steer_core::tools::capability::Capabilities;
use steer_core::tools::static_tools::{
    AstGrepTool, BashTool, DispatchAgentTool, EditTool, FetchTool, GlobTool, GrepTool, LsTool,
    MultiEditTool, ReplaceTool, TodoReadTool, TodoWriteTool, ViewTool,
};
use steer_core::tools::{DispatchAgentParams, DispatchAgentTarget, WorkspaceTarget};
use steer_tools::result::{ExternalResult, ToolResult};
use steer_tools::tools::{DISPATCH_AGENT_TOOL_NAME, LS_TOOL_NAME, TODO_READ_TOOL_NAME};
use steer_tools::{InputSchema, ToolCall, ToolSchema as Tool};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn fresh_tool_use_id() -> String {
    format!("tool_use_{}", Uuid::new_v4())
}

fn test_client() -> Client {
    dotenv().ok();
    let app_config = test_utils::test_app_config();
    Client::new_with_deps(
        app_config.llm_config_provider,
        app_config.provider_registry,
        app_config.model_registry,
    )
}

async fn default_tool_schemas() -> Vec<Tool> {
    let mut registry = ToolRegistry::new();
    registry.register_static(GrepTool);
    registry.register_static(GlobTool);
    registry.register_static(LsTool);
    registry.register_static(ViewTool);
    registry.register_static(BashTool);
    registry.register_static(EditTool);
    registry.register_static(MultiEditTool);
    registry.register_static(ReplaceTool);
    registry.register_static(AstGrepTool);
    registry.register_static(TodoReadTool);
    registry.register_static(TodoWriteTool);
    registry.register_static(DispatchAgentTool);
    registry.register_static(FetchTool);

    registry.available_schemas(Capabilities::all()).await
}

async fn run_api_with_tool_response(client: &Client, model: &ModelId) -> Result<(), ApiError> {
    println!("Testing API with tool response for model: {model:?}");

    let tool_use_id = fresh_tool_use_id();
    let ts1 = Message::current_timestamp();
    let ts2 = ts1 + 1;
    let ts3 = ts2 + 1;
    let messages = vec![
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Please list the files in the current directory using the LS tool"
                        .to_string(),
                }],
            },
            timestamp: ts1,
            id: Message::generate_id("user", ts1),
            parent_message_id: None,
        },
        Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: ToolCall {
                        id: tool_use_id.clone(),
                        name: "ls".to_string(),
                        parameters: serde_json::json!({ "path": "." }),
                    },
                    thought_signature: None,
                }],
            },
            timestamp: ts2,
            id: Message::generate_id("assistant", ts2),
            parent_message_id: Some(Message::generate_id("user", ts1)),
        },
        Message {
            data: MessageData::Tool {
                tool_use_id: tool_use_id.clone(),
                result: ToolResult::External(ExternalResult {
                    tool_name: "ls".to_string(),
                    payload: "foo.txt, bar.rs".to_string(),
                }),
            },
            timestamp: ts3,
            id: Message::generate_id("tool", ts3),
            parent_message_id: Some(Message::generate_id("assistant", ts2)),
        },
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "What was the name of the rust file?".to_string(),
                }],
            },
            timestamp: ts3 + 1,
            id: Message::generate_id("user", ts3 + 1),
            parent_message_id: Some(Message::generate_id("tool", ts3)),
        },
    ];

    let response = client
        .complete(&model, messages, None, None, None, CancellationToken::new())
        .await;

    let response = response.map_err(|e| {
        eprintln!("API call failed for model {model:?}: {e:?}");
        e
    })?;

    let final_text = response.extract_text();
    assert!(
        !final_text.is_empty(),
        "Final response text should not be empty for model {model:?}"
    );
    assert!(
        final_text.to_lowercase().contains("bar.rs"),
        "Final response for model {model:?} should mention 'bar.rs', got: '{final_text}'"
    );

    println!("Tool Response test for {model:?} passed successfully!");
    Ok(())
}

async fn run_api_with_cancelled_tool_execution(
    client: &Client,
    model: &ModelId,
) -> Result<(), ApiError> {
    println!("Testing cancelled tool execution for model: {model:?}");

    let tool_call_id = fresh_tool_use_id();

    let ts1 = Message::current_timestamp();
    let ts2 = ts1 + 1;
    let ts3 = ts2 + 1;
    let ts4 = ts3 + 1;
    let messages = vec![
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Please list the files in the current directory".to_string(),
                }],
            },
            timestamp: ts1,
            id: Message::generate_id("user", ts1),
            parent_message_id: None,
        },
        Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: ToolCall {
                        id: tool_call_id.clone(),
                        name: "ls".to_string(),
                        parameters: serde_json::json!({ "path": "." }),
                    },
                    thought_signature: None,
                }],
            },
            timestamp: ts2,
            id: Message::generate_id("assistant", ts2),
            parent_message_id: Some(Message::generate_id("user", ts1)),
        },
        Message {
            data: MessageData::Tool {
                tool_use_id: tool_call_id.clone(),
                result: ToolResult::External(ExternalResult {
                    tool_name: "ls".to_string(),
                    payload: "Tool execution was cancelled by user before completion.".to_string(),
                }),
            },
            timestamp: ts3,
            id: Message::generate_id("tool", ts3),
            parent_message_id: Some(Message::generate_id("assistant", ts2)),
        },
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "No problem, can you tell me about Rust instead?".to_string(),
                }],
            },
            timestamp: ts4,
            id: Message::generate_id("user", ts4),
            parent_message_id: Some(Message::generate_id("tool", ts3)),
        },
    ];

    let response = client
        .complete(&model, messages, None, None, None, CancellationToken::new())
        .await;

    let response = response.map_err(|e| {
        eprintln!("API call failed for model {model:?} with cancelled tool: {e:?}");
        e
    })?;

    let response_text = response.extract_text();
    assert!(
        !response_text.is_empty(),
        "Response should not be empty for model {model:?} after cancelled tool"
    );

    assert!(
        response_text.to_lowercase().contains("rust")
            || response_text.to_lowercase().contains("programming")
            || response_text.to_lowercase().contains("language"),
        "Response for model {model:?} should address the Rust question, got: '{response_text}'"
    );

    println!("Cancelled tool execution test for {model:?} passed successfully!");
    Ok(())
}

async fn run_streaming_basic(client: &Client, model: &ModelId) -> Result<(), ApiError> {
    println!("Testing streaming API for model: {model:?}");

    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "What is 2+2? Answer in one word.".to_string(),
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    let stream_result = client
        .stream_complete(&model, messages, None, None, None, CancellationToken::new())
        .await;

    let mut stream = stream_result.map_err(|e| {
        eprintln!("Streaming API call failed for model {model:?}: {e:?}");
        e
    })?;

    let mut text_chunks = Vec::new();
    let mut got_complete = false;
    let mut final_response = None;

    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::TextDelta(text) => {
                println!("{model:?} delta: {text}");
                text_chunks.push(text);
            }
            StreamChunk::ThinkingDelta(text) => {
                println!("{model:?} thinking: {text}");
            }
            StreamChunk::MessageComplete(response) => {
                println!("{model:?} complete");
                got_complete = true;
                final_response = Some(response);
            }
            StreamChunk::Error(e) => {
                eprintln!("{model:?} stream error: {e:?}");
                return Err(ApiError::StreamError {
                    provider: format!("{model:?}"),
                    details: format!("{e:?}"),
                });
            }
            _ => {}
        }
    }

    assert!(
        !text_chunks.is_empty(),
        "Should have received text deltas for model {model:?}"
    );
    assert!(
        got_complete,
        "Should have received MessageComplete for model {model:?}"
    );

    let final_text = final_response
        .as_ref()
        .map(|r| r.extract_text())
        .unwrap_or_default();
    assert!(
        final_text.contains("4") || final_text.to_lowercase().contains("four"),
        "Response for model {model:?} should contain '4', got: '{final_text}'"
    );

    println!("Streaming API test for {model:?} passed successfully!");
    Ok(())
}

async fn run_streaming_with_tools(client: &Client, model: &ModelId) -> Result<(), ApiError> {
    println!("Testing streaming API with tools for model: {model:?}");

    let temp_dir = TempDir::new().unwrap();
    let tools = default_tool_schemas().await;
    // Ensure both a nested-schema tool and a zero-arg tool remain present in streamed tool sets.
    assert!(
        tools.iter().any(|tool| tool.name == DISPATCH_AGENT_TOOL_NAME),
        "Expected dispatch_agent tool schema to be present"
    );
    assert!(
        tools.iter().any(|tool| tool.name == TODO_READ_TOOL_NAME),
        "Expected read_todos tool schema to be present"
    );
    let pwd_display = temp_dir.path().display().to_string();

    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: format!(
                    "You must call the {LS_TOOL_NAME} tool with path \"{pwd_display}\" exactly once. Do not call any other tools. Do not answer with text before the tool call."
                ),
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    let stream_result = client
        .stream_complete(
            &model,
            messages,
            None,
            Some(tools),
            None,
            CancellationToken::new(),
        )
        .await;

    let mut stream = stream_result.map_err(|e| {
        eprintln!("Streaming API call failed for model {model:?}: {e:?}");
        e
    })?;

    let mut tool_starts = Vec::new();
    let mut tool_deltas = Vec::new();
    let mut got_complete = false;
    let mut final_response = None;

    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::ToolUseStart { id, name } => {
                println!("{model:?} tool start: {name} (id: {id})");
                tool_starts.push((id, name));
            }
            StreamChunk::ToolUseInputDelta { id, delta } => {
                println!("{model:?} tool delta: {delta}");
                tool_deltas.push((id, delta));
            }
            StreamChunk::TextDelta(text) => {
                println!("{model:?} text delta: {text}");
            }
            StreamChunk::MessageComplete(response) => {
                println!("{model:?} complete");
                got_complete = true;
                final_response = Some(response);
            }
            StreamChunk::Error(e) => {
                eprintln!("{model:?} stream error: {e:?}");
                return Err(ApiError::StreamError {
                    provider: format!("{model:?}"),
                    details: format!("{e:?}"),
                });
            }
            _ => {}
        }
    }

    assert!(
        !tool_starts.is_empty(),
        "Should have received ToolUseStart for model {model:?}"
    );
    assert!(
        got_complete,
        "Should have received MessageComplete for model {model:?}"
    );

    let response = final_response.expect("Should have final response");
    assert!(
        response.has_tool_calls(),
        "Final response for model {model:?} should contain tool calls"
    );
    let tool_calls = response.extract_tool_calls();
    assert!(
        tool_calls.iter().any(|tc| tc.name == "ls"),
        "Should have an ls tool call for model {model:?}"
    );

    println!("Streaming API with tools test for {model:?} passed successfully!");
    Ok(())
}

async fn run_streaming_with_reasoning(client: &Client, model: &ModelId) -> Result<(), ApiError> {
    println!("Testing streaming API with reasoning for model: {model:?}");

    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "What is the sum of the first 10 prime numbers? Think step by step."
                    .to_string(),
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    let stream_result = client
        .stream_complete(&model, messages, None, None, None, CancellationToken::new())
        .await;

    let mut stream = stream_result.map_err(|e| {
        eprintln!("Streaming API call failed for model {model:?}: {e:?}");
        e
    })?;

    let mut thinking_chunks = Vec::new();
    let mut text_chunks = Vec::new();
    let mut got_complete = false;

    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::ThinkingDelta(text) => {
                println!(
                    "{model:?} thinking: {}",
                    text.chars().take(50).collect::<String>()
                );
                thinking_chunks.push(text);
            }
            StreamChunk::TextDelta(text) => {
                println!("{model:?} text delta: {text}");
                text_chunks.push(text);
            }
            StreamChunk::MessageComplete(_) => {
                println!("{model:?} complete");
                got_complete = true;
            }
            StreamChunk::Error(e) => {
                eprintln!("{model:?} stream error: {e:?}");
                return Err(ApiError::StreamError {
                    provider: format!("{model:?}"),
                    details: format!("{e:?}"),
                });
            }
            _ => {}
        }
    }

    assert!(
        got_complete,
        "Should have received MessageComplete for model {model:?}"
    );
    let got_content = !thinking_chunks.is_empty() || !text_chunks.is_empty();
    assert!(
        got_content,
        "Should have received thinking or text content for model {model:?}"
    );

    println!(
        "Streaming with reasoning test for {model:?} passed! Thinking chunks: {}, Text chunks: {}",
        thinking_chunks.len(),
        text_chunks.len()
    );
    Ok(())
}

async fn run_api_with_dispatch_agent_tool_call(
    client: &Client,
    model: &ModelId,
) -> Result<(), ApiError> {
    println!("Testing dispatch_agent tool call for model: {model:?}");

    let tools = default_tool_schemas().await;
    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "You must call the dispatch_agent tool exactly once. Use the prompt \"find files\", target a new session in the current workspace, and use the explore agent. Do not call any other tools. Do not answer with text before the tool call.".to_string(),
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    let response = client
        .complete(
            model,
            messages,
            None,
            Some(tools),
            None,
            CancellationToken::new(),
        )
        .await;

    let response = response.map_err(|e| {
        eprintln!("API call failed for model {model:?} dispatch_agent test: {e:?}");
        e
    })?;

    assert!(
        response.has_tool_calls(),
        "Final response for model {model:?} should contain tool calls"
    );

    let tool_call = response
        .extract_tool_calls()
        .into_iter()
        .find(|tool_call| tool_call.name == DISPATCH_AGENT_TOOL_NAME)
        .expect("Expected dispatch_agent tool call");

    let mut params_value = match &tool_call.parameters {
        Value::String(raw) => serde_json::from_str(raw)
            .unwrap_or_else(|err| panic!("dispatch_agent params string should be JSON: {err}")),
        _ => tool_call.parameters.clone(),
    };

    if let Some(target_raw) = params_value
        .get("target")
        .and_then(|value| value.as_str())
    {
        let target_value = serde_json::from_str::<Value>(target_raw).unwrap_or_else(|err| {
            panic!("dispatch_agent target string should be JSON: {err}")
        });
        if let Some(obj) = params_value.as_object_mut() {
            obj.insert("target".to_string(), target_value);
        }
    }

    let params: DispatchAgentParams =
        serde_json::from_value(params_value).expect("dispatch_agent params");

    assert_eq!(params.prompt, "find files");

    match params.target {
        DispatchAgentTarget::New { workspace, agent } => {
            assert!(matches!(workspace, WorkspaceTarget::Current));
            if let Some(agent) = agent.as_deref() {
                assert_eq!(agent, "explore");
            }
        }
        DispatchAgentTarget::Resume { .. } => {
            panic!("Expected new dispatch_agent target");
        }
    }

    println!("dispatch_agent tool call test for {model:?} passed successfully!");
    Ok(())
}

#[tokio::test]
async fn test_tool_schemas_include_object_properties() {
    let tools = default_tool_schemas().await;

    let dispatch_tool = tools
        .iter()
        .find(|tool| tool.name == DISPATCH_AGENT_TOOL_NAME)
        .expect("dispatch_agent schema should be registered");
    let dispatch_schema = dispatch_tool.input_schema.as_value();
    let dispatch_type = dispatch_schema.get("type").and_then(|value| value.as_str());
    assert_eq!(dispatch_type, Some("object"));
    let dispatch_properties = dispatch_schema
        .get("properties")
        .and_then(|value| value.as_object())
        .expect("dispatch_agent schema should include properties");
    assert!(dispatch_properties.contains_key("prompt"));
    assert!(dispatch_properties.contains_key("target"));

    let todo_tool = tools
        .iter()
        .find(|tool| tool.name == TODO_READ_TOOL_NAME)
        .expect("read_todos schema should be registered");
    let todo_schema = todo_tool.input_schema.as_value();
    let todo_type = todo_schema.get("type").and_then(|value| value.as_str());
    assert_eq!(todo_type, Some("object"));
    let todo_properties = todo_schema
        .get("properties")
        .and_then(|value| value.as_object())
        .expect("read_todos schema should include properties");
    assert!(todo_properties.is_empty());
}

#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY environment variable"]
async fn test_openai_responses_stream_tool_call_ids_non_empty() {
    dotenv().ok();
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let client = steer_core::api::openai::OpenAIClient::with_mode(
        api_key,
        steer_core::api::openai::OpenAIMode::Responses,
    );

    let tools = default_tool_schemas().await;

    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: format!(
                    "You must call the {LS_TOOL_NAME} tool with path '.' exactly once. Do not call any other tools. Do not answer with text before the tool call."
                ),
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    let model_id = steer_core::config::model::ModelId::new(
        steer_core::config::provider::openai(),
        "gpt-4.1-mini-2025-04-14",
    );

    let mut stream = client
        .stream_complete(
            &model_id,
            messages,
            None,
            Some(tools),
            None,
            CancellationToken::new(),
        )
        .await
        .expect("stream_complete should succeed");

    let mut saw_tool_start = false;
    let mut saw_tool_delta = false;
    let mut saw_tool_call = false;
    let mut saw_empty_id = false;
    let mut saw_empty_name = false;

    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::ToolUseStart { id, name } => {
                saw_tool_start = true;
                if id.is_empty() {
                    saw_empty_id = true;
                }
                if name.is_empty() {
                    saw_empty_name = true;
                }
            }
            StreamChunk::ToolUseInputDelta { id, .. } => {
                saw_tool_delta = true;
                if id.is_empty() {
                    saw_empty_id = true;
                }
            }
            StreamChunk::MessageComplete(response) => {
                for item in response.content {
                    if let AssistantContent::ToolCall { tool_call, .. } = item {
                        saw_tool_call = true;
                        if tool_call.id.is_empty() {
                            saw_empty_id = true;
                        }
                        if tool_call.name.is_empty() {
                            saw_empty_name = true;
                        }
                    }
                }
                break;
            }
            StreamChunk::Error(err) => {
                panic!("stream error: {err:?}");
            }
            _ => {}
        }
    }

    assert!(
        saw_tool_start || saw_tool_call,
        "Expected at least one tool call in stream"
    );
    assert!(
        saw_tool_delta || saw_tool_call,
        "Expected tool args in stream"
    );
    assert!(!saw_empty_id, "Tool call id should never be empty");
    assert!(!saw_empty_name, "Tool call name should never be empty");
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore]
async fn test_api_with_tool_response(#[case] model: ModelId) {
    let client = test_client();
    run_api_with_tool_response(&client, &model)
        .await
        .unwrap_or_else(|err| panic!("tool response test failed for {model:?}: {err:?}"));
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore]
async fn test_api_dispatch_agent_tool_call(#[case] model: ModelId) {
    let client = test_client();
    run_api_with_dispatch_agent_tool_call(&client, &model)
        .await
        .unwrap_or_else(|err| panic!("dispatch_agent tool call test failed for {model:?}: {err:?}"));
}

#[tokio::test]
#[ignore]
async fn test_gemini_system_instructions() {
    dotenv().ok();
    let app_config = test_utils::test_app_config();
    let client = Client::new_with_deps(
        app_config.llm_config_provider,
        app_config.provider_registry,
        app_config.model_registry,
    );

    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "What's your name?".to_string(),
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    let system = Some(SystemContext::new(
        "Your name is GeminiHelper. Always introduce yourself as GeminiHelper.".to_string(),
    ));

    let response = client
        .complete(
            &steer_core::config::model::builtin::gemini_3_flash_preview(), // Use Gemini model
            messages,
            system,
            None,
            None,
            CancellationToken::new(),
        )
        .await;

    assert!(response.is_ok(), "API call failed: {:?}", response.err());

    let response = response.unwrap();

    let text = response.extract_text();
    println!("Gemini response: {text}");

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
    let app_config = test_utils::test_app_config();
    let client = Client::new_with_deps(
        app_config.llm_config_provider,
        app_config.provider_registry,
        app_config.model_registry,
    );

    let ts1 = Message::current_timestamp();
    let ts2 = ts1 + 1;
    let ts3 = ts2 + 1;
    let messages = vec![
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Please list the files in the current directory using the LS tool"
                        .to_string(),
                }],
            },
            timestamp: ts1,
            id: Message::generate_id("user", ts1),
            parent_message_id: None,
        },
        Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: ToolCall {
                        id: "tool-use-id-error".to_string(),
                        name: "ls".to_string(),
                        parameters: serde_json::json!({ "path": "." }),
                    },
                    thought_signature: None,
                }],
            },
            timestamp: ts2,
            id: Message::generate_id("assistant", ts2),
            parent_message_id: Some(Message::generate_id("user", ts1)),
        },
        Message {
            data: MessageData::Tool {
                tool_use_id: "tool-use-id-error".to_string(),
                result: ToolResult::Error(steer_tools::ToolError::Execution(
                    steer_tools::error::ToolExecutionError::External {
                        tool_name: "ls".to_string(),
                        message: "Error executing command".to_string(),
                    },
                )),
            },
            timestamp: ts3,
            id: Message::generate_id("tool", ts3),
            parent_message_id: Some(Message::generate_id("assistant", ts2)),
        },
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Okay, thank you.".to_string(),
                }],
            },
            timestamp: ts3 + 1,
            id: Message::generate_id("user", ts3 + 1),
            parent_message_id: Some(Message::generate_id("tool", ts3)),
        },
    ];

    let response = client
        .complete(
            &steer_core::config::model::builtin::gemini_3_flash_preview(), // Use Gemini model
            messages,
            None,
            None, // No tools needed here as we are providing the tool result
            None,
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
    let app_config = test_utils::test_app_config();
    let client = Client::new_with_deps(
        app_config.llm_config_provider,
        app_config.provider_registry,
        app_config.model_registry,
    );

    // Define a tool with a complex schema
    let complex_tool = Tool {
        name: "complex_operation".to_string(),
        display_name: "Complex Operation".to_string(),
        description: "Performs a complex operation with nested parameters.".to_string(),
        input_schema: InputSchema::object(
            serde_json::map::Map::from_iter(vec![
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
            vec!["config".to_string(), "items".to_string()],
        ),
    };

    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "What is the weather today?".to_string(), // A simple message, doesn't need to invoke the tool
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    let response = client
        .complete(
            &steer_core::config::model::builtin::gemini_3_flash_preview(),
            messages,
            None,
            Some(vec![complex_tool]), // Send the complex tool definition
            None,
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
    let app_config = test_utils::test_app_config();
    let client = Client::new_with_deps(
        app_config.llm_config_provider,
        app_config.provider_registry,
        app_config.model_registry,
    );

    // Define the JSON string to be used as the tool result content
    let json_result_string =
        serde_json::json!({ "status": "success", "files": ["file1.txt", "file2.log"] }).to_string();

    let ts1 = Message::current_timestamp();
    let ts2 = ts1 + 1;
    let ts3 = ts2 + 1;
    let messages = vec![
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Run the file listing tool.".to_string(),
                }],
            },
            timestamp: ts1,
            id: Message::generate_id("user", ts1),
            parent_message_id: None,
        },
        Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: ToolCall {
                        id: "tool-use-id-json".to_string(),
                        name: "list_files".to_string(),
                        parameters: serde_json::json!({}), // Empty input for simplicity
                    },
                    thought_signature: None,
                }],
            },
            timestamp: ts2,
            id: Message::generate_id("assistant", ts2),
            parent_message_id: Some(Message::generate_id("user", ts1)),
        },
        Message {
            data: MessageData::Tool {
                tool_use_id: "tool-use-id-json".to_string(),
                result: ToolResult::External(ExternalResult {
                    tool_name: "list_files".to_string(),
                    payload: json_result_string,
                }),
            },
            timestamp: ts3,
            id: Message::generate_id("tool", ts3),
            parent_message_id: Some(Message::generate_id("assistant", ts2)),
        },
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Thanks for the list.".to_string(),
                }],
            },
            timestamp: ts3 + 1,
            id: Message::generate_id("user", ts3 + 1),
            parent_message_id: Some(Message::generate_id("tool", ts3)),
        },
    ];

    let response = client
        .complete(
            &steer_core::config::model::builtin::gemini_3_flash_preview(), // Use Gemini model
            messages,
            None,
            None, // No tools needed here as we are providing the tool result
            None,
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
    let app_config = test_utils::test_app_config();
    let client = Client::new_with_deps(
        app_config.llm_config_provider,
        app_config.provider_registry,
        app_config.model_registry,
    );

    let ts1 = Message::current_timestamp();
    let ts2 = ts1 + 1;
    let ts3 = ts2 + 1;
    let ts4 = ts3 + 1;
    let messages = vec![
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Please list files in '.' and check the weather in 'SF'".to_string(),
                }],
            },
            timestamp: ts1,
            id: Message::generate_id("user", ts1),
            parent_message_id: None,
        },
        // Assistant makes two tool calls
        Message {
            data: MessageData::Assistant {
                content: vec![
                    AssistantContent::ToolCall {
                        tool_call: ToolCall {
                            id: "tool-use-id-1".to_string(),
                            name: "ls".to_string(),
                            parameters: serde_json::json!({ "path": "." }),
                        },
                        thought_signature: None,
                    },
                    AssistantContent::ToolCall {
                        tool_call: ToolCall {
                            id: "tool-use-id-2".to_string(),
                            name: "get_weather".to_string(),
                            parameters: serde_json::json!({ "location": "SF" }),
                        },
                        thought_signature: None,
                    },
                ],
            },
            timestamp: ts2,
            id: Message::generate_id("assistant", ts2),
            parent_message_id: Some(Message::generate_id("user", ts1)),
        },
        // Provide results for both tool calls
        Message {
            data: MessageData::Tool {
                tool_use_id: "tool-use-id-1".to_string(),
                result: ToolResult::External(ExternalResult {
                    tool_name: "ls".to_string(),
                    payload: "file1.rs, file2.toml".to_string(),
                }),
            },
            timestamp: ts3,
            id: Message::generate_id("tool", ts3),
            parent_message_id: Some(Message::generate_id("assistant", ts2)),
        },
        Message {
            data: MessageData::Tool {
                tool_use_id: "tool-use-id-2".to_string(),
                result: ToolResult::External(ExternalResult {
                    tool_name: "get_weather".to_string(),
                    payload: "Sunny, 20C".to_string(),
                }),
            },
            timestamp: ts4,
            id: Message::generate_id("tool", ts4),
            parent_message_id: Some(Message::generate_id("assistant", ts2)),
        },
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Got it, thanks!".to_string(),
                }],
            },
            timestamp: ts4 + 1,
            id: Message::generate_id("user", ts4 + 1),
            parent_message_id: Some(Message::generate_id("tool", ts4)),
        },
    ];

    // Define the 'get_weather' tool for the API call, 'ls' is usually predefined
    let weather_tool = Tool {
        name: "get_weather".to_string(),
        display_name: "Get Weather".to_string(),
        description: "Gets the weather for a location".to_string(),
        input_schema: InputSchema::object(
            serde_json::map::Map::from_iter(vec![(
                "location".to_string(),
                json!({"type": "string", "description": "The location to get weather for"}),
            )]),
            vec!["location".to_string()],
        ),
    };
    // Get available tools
    let mut tools = default_tool_schemas().await;
    tools.push(weather_tool);

    let response = client
        .complete(
            &steer_core::config::model::builtin::gemini_3_flash_preview(), // Use Gemini model
            messages,
            None,
            Some(tools), // Provide tools including the dummy weather tool
            None,
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

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore]
async fn test_api_with_cancelled_tool_execution(#[case] model: ModelId) {
    let client = test_client();
    run_api_with_cancelled_tool_execution(&client, &model)
        .await
        .unwrap_or_else(|err| panic!("cancelled tool test failed for {model:?}: {err:?}"));
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore]
async fn test_api_streaming_basic(#[case] model: ModelId) {
    let client = test_client();
    run_streaming_basic(&client, &model)
        .await
        .unwrap_or_else(|err| panic!("streaming basic test failed for {model:?}: {err:?}"));
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore]
async fn test_api_streaming_with_tools(#[case] model: ModelId) {
    let client = test_client();
    run_streaming_with_tools(&client, &model)
        .await
        .unwrap_or_else(|err| panic!("streaming tools test failed for {model:?}: {err:?}"));
}

#[rstest]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[tokio::test]
#[ignore]
async fn test_api_streaming_with_reasoning(#[case] model: ModelId) {
    let client = test_client();
    run_streaming_with_reasoning(&client, &model)
        .await
        .unwrap_or_else(|err| panic!("streaming reasoning test failed for {model:?}: {err:?}"));
}

#[tokio::test]
#[ignore]
async fn test_api_streaming_cancellation() {
    dotenv().ok();
    let app_config = test_utils::test_app_config();
    let client = Client::new_with_deps(
        app_config.llm_config_provider,
        app_config.provider_registry,
        app_config.model_registry,
    );

    // Test cancellation with a model that generates longer responses
    let model = builtin::claude_haiku_4_5();

    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "Write a 500 word essay about the history of computing.".to_string(),
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    let token = CancellationToken::new();
    let token_clone = token.clone();

    let stream_result = client
        .stream_complete(&model, messages, None, None, None, token)
        .await;

    let mut stream = stream_result.expect("Should get stream");

    let mut chunks_before_cancel = 0;
    let mut got_cancelled = false;

    // Consume a few chunks then cancel
    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::TextDelta(_) => {
                chunks_before_cancel += 1;
                // Cancel after receiving a few chunks
                if chunks_before_cancel >= 3 {
                    println!("Cancelling stream after {chunks_before_cancel} chunks");
                    token_clone.cancel();
                }
            }
            StreamChunk::Error(steer_core::api::StreamError::Cancelled) => {
                println!("Received cancellation signal");
                got_cancelled = true;
                break;
            }
            StreamChunk::MessageComplete(_) => {
                // If we got complete before cancellation took effect, that's okay
                println!("Got complete before cancellation took effect");
                break;
            }
            StreamChunk::Error(e) => {
                // Some providers may return different errors on cancellation
                println!("Got error during cancellation: {e:?}");
                break;
            }
            _ => {}
        }
    }

    assert!(
        chunks_before_cancel > 0,
        "Should have received at least one chunk before cancellation"
    );

    println!(
        "Cancellation test passed! Chunks before cancel: {chunks_before_cancel}, Got cancelled signal: {got_cancelled}"
    );
}
