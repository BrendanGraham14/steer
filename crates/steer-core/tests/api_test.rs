use dotenvy::dotenv;
use futures::StreamExt;
use rstest::rstest;
use steer_core::api::{ApiError, Client, Provider, StreamChunk};
use steer_core::app::SystemContext;
use steer_core::app::conversation::{
    AssistantContent, ImageContent, ImageSource, Message, MessageData, UserContent,
};
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
use steer_tools::result::{EditResult, ExternalResult, FileContentResult, ToolResult};
use steer_tools::tools::{DISPATCH_AGENT_TOOL_NAME, LS_TOOL_NAME, TODO_READ_TOOL_NAME};
use steer_tools::{InputSchema, ToolCall, ToolSchema as Tool};
use tempfile::TempDir;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type ApiTestResult<T> = Result<T, ApiTestError>;

#[derive(Debug, Error)]
enum ApiTestError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("failed to create temp dir: {0}")]
    TempDir(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("missing expected {0}")]
    Missing(&'static str),
    #[error("unexpected dispatch_agent target")]
    UnexpectedDispatchAgentTarget,
}

fn fresh_tool_use_id() -> String {
    format!("tool_use_{}", Uuid::new_v4())
}

fn tiny_png_data_url() -> &'static str {
    "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAADklEQVR42mP4DwYMEAoAU7oL9YXEbhEAAAAASUVORK5CYII="
}

fn build_image_message(source: ImageSource) -> Message {
    let timestamp = Message::current_timestamp();
    Message {
        data: MessageData::User {
            content: vec![
                UserContent::Text {
                    text: "Please describe this image in one short sentence.".to_string(),
                },
                UserContent::Image {
                    image: ImageContent {
                        mime_type: "image/png".to_string(),
                        source,
                        width: Some(1),
                        height: Some(1),
                        bytes: None,
                        sha256: None,
                    },
                },
            ],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }
}

fn assert_usage_invariants_if_present(
    response: &steer_core::api::CompletionResponse,
    model: &ModelId,
) {
    let usage_expected = matches!(
        model.provider.as_str(),
        "openai" | "anthropic" | "google" | "xai"
    );

    if usage_expected {
        assert!(
            response.usage.is_some(),
            "expected usage to be present for model {model:?}"
        );
    }

    if let Some(usage) = response.usage {
        assert!(
            usage.total_tokens >= usage.input_tokens,
            "total tokens should be >= input tokens for model {model:?}, usage={usage:?}"
        );
        assert!(
            usage.total_tokens >= usage.output_tokens,
            "total tokens should be >= output tokens for model {model:?}, usage={usage:?}"
        );
    }
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

async fn run_api_with_data_url_image(client: &Client, model: &ModelId) -> ApiTestResult<()> {
    let messages = vec![build_image_message(ImageSource::DataUrl {
        data_url: tiny_png_data_url().to_string(),
    })];

    let response = client
        .complete(model, messages, None, None, None, CancellationToken::new())
        .await?;

    let final_text = response.extract_text();
    assert!(
        !final_text.trim().is_empty(),
        "Image response text should not be empty for model {model:?}"
    );

    Ok(())
}

async fn run_api_with_tool_response(client: &Client, model: &ModelId) -> ApiTestResult<()> {
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
        .complete(model, messages, None, None, None, CancellationToken::new())
        .await?;

    assert_usage_invariants_if_present(&response, model);

    let final_text = response.extract_text();
    assert!(
        !final_text.is_empty(),
        "Final response text should not be empty for model {model:?}"
    );
    assert!(
        final_text.to_lowercase().contains("bar.rs"),
        "Final response for model {model:?} should mention 'bar.rs', got: '{final_text}'"
    );

    Ok(())
}

async fn run_api_with_cancelled_tool_execution(
    client: &Client,
    model: &ModelId,
) -> ApiTestResult<()> {
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
        .complete(model, messages, None, None, None, CancellationToken::new())
        .await?;

    assert_usage_invariants_if_present(&response, model);

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

    Ok(())
}

async fn run_api_with_pruned_duplicate_read_file_history(
    client: &Client,
    model: &ModelId,
) -> ApiTestResult<()> {
    let tool_call_id = fresh_tool_use_id();

    let ts1 = Message::current_timestamp();
    let ts2 = ts1 + 1;
    let ts3 = ts2 + 1;
    let ts4 = ts3 + 1;
    let messages = vec![
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Use the tool output below to answer the question.".to_string(),
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
                        name: "read_file".to_string(),
                        parameters: serde_json::json!({
                            "file_path": "/tmp/pruned-config.toml",
                            "offset": 100,
                            "limit": 200
                        }),
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
                tool_use_id: tool_call_id,
                result: ToolResult::FileContent(FileContentResult {
                    content: "name = \"demo\"\nstatus = \"green\"\nversion = 3".to_string(),
                    file_path: "/tmp/pruned-config.toml".to_string(),
                    line_count: 3,
                    truncated: false,
                }),
            },
            timestamp: ts3,
            id: Message::generate_id("tool", ts3),
            parent_message_id: Some(Message::generate_id("assistant", ts2)),
        },
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "What is the status value? Reply with just the value.".to_string(),
                }],
            },
            timestamp: ts4,
            id: Message::generate_id("user", ts4),
            parent_message_id: Some(Message::generate_id("tool", ts3)),
        },
    ];

    let response = client
        .complete(model, messages, None, None, None, CancellationToken::new())
        .await?;

    assert_usage_invariants_if_present(&response, model);

    let response_text = response.extract_text();
    assert!(
        response_text.to_lowercase().contains("green"),
        "response for model {model:?} should mention 'green', got: '{response_text}'"
    );

    Ok(())
}

async fn run_api_with_pruned_pre_edit_read_history(
    client: &Client,
    model: &ModelId,
) -> ApiTestResult<()> {
    let edit_call_id = fresh_tool_use_id();
    let read_call_id = fresh_tool_use_id();

    let ts1 = Message::current_timestamp();
    let ts2 = ts1 + 1;
    let ts3 = ts2 + 1;
    let ts4 = ts3 + 1;
    let ts5 = ts4 + 1;
    let ts6 = ts5 + 1;
    let messages = vec![
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Confirm the final GREETING constant after the edit.".to_string(),
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
                        id: edit_call_id.clone(),
                        name: "edit_file".to_string(),
                        parameters: serde_json::json!({
                            "file_path": "/tmp/pruned-greeting.rs",
                            "old_string": "const GREETING: &str = \"hello\";",
                            "new_string": "const GREETING: &str = \"bonjour\";"
                        }),
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
                tool_use_id: edit_call_id,
                result: ToolResult::Edit(EditResult {
                    file_path: "/tmp/pruned-greeting.rs".to_string(),
                    changes_made: 1,
                    file_created: false,
                    old_content: None,
                    new_content: None,
                }),
            },
            timestamp: ts3,
            id: Message::generate_id("tool", ts3),
            parent_message_id: Some(Message::generate_id("assistant", ts2)),
        },
        Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: ToolCall {
                        id: read_call_id.clone(),
                        name: "read_file".to_string(),
                        parameters: serde_json::json!({
                            "file_path": "/tmp/pruned-greeting.rs",
                            "offset": 1,
                            "limit": 200
                        }),
                    },
                    thought_signature: None,
                }],
            },
            timestamp: ts4,
            id: Message::generate_id("assistant", ts4),
            parent_message_id: Some(Message::generate_id("tool", ts3)),
        },
        Message {
            data: MessageData::Tool {
                tool_use_id: read_call_id,
                result: ToolResult::FileContent(FileContentResult {
                    content: "const GREETING: &str = \"bonjour\";".to_string(),
                    file_path: "/tmp/pruned-greeting.rs".to_string(),
                    line_count: 1,
                    truncated: false,
                }),
            },
            timestamp: ts5,
            id: Message::generate_id("tool", ts5),
            parent_message_id: Some(Message::generate_id("assistant", ts4)),
        },
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "What is GREETING now? Reply with just the value.".to_string(),
                }],
            },
            timestamp: ts6,
            id: Message::generate_id("user", ts6),
            parent_message_id: Some(Message::generate_id("tool", ts5)),
        },
    ];

    let response = client
        .complete(model, messages, None, None, None, CancellationToken::new())
        .await?;

    assert_usage_invariants_if_present(&response, model);

    let response_text = response.extract_text();
    assert!(
        response_text.to_lowercase().contains("bonjour"),
        "response for model {model:?} should mention 'bonjour', got: '{response_text}'"
    );

    Ok(())
}

async fn run_api_with_pruned_assistant_text_and_tool_history(
    client: &Client,
    model: &ModelId,
) -> ApiTestResult<()> {
    let tool_call_id = fresh_tool_use_id();

    let ts1 = Message::current_timestamp();
    let ts2 = ts1 + 1;
    let ts3 = ts2 + 1;
    let ts4 = ts3 + 1;
    let ts5 = ts4 + 1;
    let messages = vec![
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Please summarize the latest command recommendation from notes."
                        .to_string(),
                }],
            },
            timestamp: ts1,
            id: Message::generate_id("user", ts1),
            parent_message_id: None,
        },
        Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "I dropped stale tool history and kept the latest notes read."
                        .to_string(),
                }],
            },
            timestamp: ts2,
            id: Message::generate_id("assistant", ts2),
            parent_message_id: Some(Message::generate_id("user", ts1)),
        },
        Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: ToolCall {
                        id: tool_call_id.clone(),
                        name: "read_file".to_string(),
                        parameters: serde_json::json!({
                            "file_path": "/tmp/notes.md",
                            "offset": 1,
                            "limit": 200
                        }),
                    },
                    thought_signature: None,
                }],
            },
            timestamp: ts3,
            id: Message::generate_id("assistant", ts3),
            parent_message_id: Some(Message::generate_id("assistant", ts2)),
        },
        Message {
            data: MessageData::Tool {
                tool_use_id: tool_call_id,
                result: ToolResult::FileContent(FileContentResult {
                    content: "recommended_command = \"cargo check --all-features\"".to_string(),
                    file_path: "/tmp/notes.md".to_string(),
                    line_count: 1,
                    truncated: false,
                }),
            },
            timestamp: ts4,
            id: Message::generate_id("tool", ts4),
            parent_message_id: Some(Message::generate_id("assistant", ts3)),
        },
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "What command was recommended? Return exactly the command.".to_string(),
                }],
            },
            timestamp: ts5,
            id: Message::generate_id("user", ts5),
            parent_message_id: Some(Message::generate_id("tool", ts4)),
        },
    ];

    let response = client
        .complete(model, messages, None, None, None, CancellationToken::new())
        .await?;

    assert_usage_invariants_if_present(&response, model);

    let response_text = response.extract_text();
    assert!(
        response_text.contains("cargo check --all-features"),
        "response for model {model:?} should contain the command, got: '{response_text}'"
    );

    Ok(())
}

async fn run_streaming_basic(client: &Client, model: &ModelId) -> ApiTestResult<()> {
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

    let mut stream = client
        .stream_complete(model, messages, None, None, None, CancellationToken::new())
        .await?;

    let mut text_chunks = Vec::new();
    let mut got_complete = false;
    let mut final_response = None;

    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::TextDelta(text) => {
                text_chunks.push(text);
            }
            StreamChunk::ThinkingDelta(_) => {}
            StreamChunk::MessageComplete(response) => {
                assert_usage_invariants_if_present(&response, model);
                got_complete = true;
                final_response = Some(response);
            }
            StreamChunk::Error(e) => {
                return Err(ApiTestError::Api(ApiError::StreamError {
                    provider: format!("{model:?}"),
                    details: format!("{e:?}"),
                }));
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
        final_text.contains('4') || final_text.to_lowercase().contains("four"),
        "Response for model {model:?} should contain '4', got: '{final_text}'"
    );

    Ok(())
}

async fn run_streaming_with_tools(client: &Client, model: &ModelId) -> ApiTestResult<()> {
    let temp_dir = TempDir::new()?;
    let tools = default_tool_schemas().await;
    // Ensure both a nested-schema tool and a zero-arg tool remain present in streamed tool sets.
    assert!(
        tools
            .iter()
            .any(|tool| tool.name == DISPATCH_AGENT_TOOL_NAME),
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

    let mut stream = client
        .stream_complete(
            model,
            messages,
            None,
            Some(tools),
            None,
            CancellationToken::new(),
        )
        .await?;

    let mut tool_starts = Vec::new();
    let mut tool_deltas = Vec::new();
    let mut got_complete = false;
    let mut final_response = None;

    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::ToolUseStart { id, name } => {
                tool_starts.push((id, name));
            }
            StreamChunk::ToolUseInputDelta { id, delta } => {
                tool_deltas.push((id, delta));
            }
            StreamChunk::TextDelta(_) => {}
            StreamChunk::MessageComplete(response) => {
                assert_usage_invariants_if_present(&response, model);
                got_complete = true;
                final_response = Some(response);
            }
            StreamChunk::Error(e) => {
                return Err(ApiTestError::Api(ApiError::StreamError {
                    provider: format!("{model:?}"),
                    details: format!("{e:?}"),
                }));
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

    let response = final_response.ok_or(ApiTestError::Missing("final response"))?;
    assert!(
        response.has_tool_calls(),
        "Final response for model {model:?} should contain tool calls"
    );
    let tool_calls = response.extract_tool_calls();
    assert!(
        tool_calls.iter().any(|tc| tc.name == "ls"),
        "Should have an ls tool call for model {model:?}"
    );

    Ok(())
}

async fn run_streaming_with_reasoning(client: &Client, model: &ModelId) -> ApiTestResult<()> {
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

    let mut stream = client
        .stream_complete(model, messages, None, None, None, CancellationToken::new())
        .await?;

    let mut thinking_chunks = Vec::new();
    let mut text_chunks = Vec::new();
    let mut got_complete = false;

    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::ThinkingDelta(text) => {
                thinking_chunks.push(text);
            }
            StreamChunk::TextDelta(text) => {
                text_chunks.push(text);
            }
            StreamChunk::MessageComplete(response) => {
                assert_usage_invariants_if_present(&response, model);
                got_complete = true;
            }
            StreamChunk::Error(e) => {
                return Err(ApiTestError::Api(ApiError::StreamError {
                    provider: format!("{model:?}"),
                    details: format!("{e:?}"),
                }));
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

    Ok(())
}

async fn run_api_with_dispatch_agent_tool_call(
    client: &Client,
    model: &ModelId,
) -> ApiTestResult<()> {
    let tools = default_tool_schemas().await;
    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "You must call the dispatch_agent tool exactly once. Use prompt \"find files\". The target must be an object with session=\"new\", workspace as an object with location=\"current\", and agent=\"explore\". Do not encode any object as a JSON string. Do not call any other tools. Do not answer with text before the tool call.".to_string(),
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
        .await?;

    assert_usage_invariants_if_present(&response, model);

    assert!(
        response.has_tool_calls(),
        "Final response for model {model:?} should contain tool calls"
    );

    let tool_call = response
        .extract_tool_calls()
        .into_iter()
        .find(|tool_call| tool_call.name == DISPATCH_AGENT_TOOL_NAME)
        .ok_or(ApiTestError::Missing("dispatch_agent tool call"))?;

    let params = serde_json::from_value::<DispatchAgentParams>(tool_call.parameters.clone())?;

    assert_eq!(params.prompt, "find files");

    match params.target {
        DispatchAgentTarget::New { workspace, agent } => {
            assert_eq!(workspace, WorkspaceTarget::Current);
            assert!(
                agent == Some("explore".to_string()) || agent.is_none(),
                "Expected agent to be Some(\"explore\") or None, got: {agent:?}"
            );
        }
        DispatchAgentTarget::Resume { .. } => {
            return Err(ApiTestError::UnexpectedDispatchAgentTarget);
        }
    }

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
async fn test_openai_responses_stream_usage_is_non_zero_when_present() {
    dotenv().ok();
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let client = steer_core::api::openai::OpenAIClient::with_mode(
        api_key,
        steer_core::api::openai::OpenAIMode::Responses,
    )
    .expect("openai client");

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

    let model_id = builtin::gpt_5_nano_2025_08_07();

    let mut stream = client
        .stream_complete(
            &model_id,
            messages,
            None,
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .expect("stream_complete should succeed");

    let mut final_response = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::MessageComplete(response) => {
                final_response = Some(response);
                break;
            }
            StreamChunk::Error(err) => panic!("stream error: {err:?}"),
            _ => {}
        }
    }

    let response = final_response.expect("expected final response");
    let usage = response
        .usage
        .expect("expected usage for OpenAI Responses completion");

    assert!(usage.input_tokens > 0, "expected non-zero input tokens");
    assert!(usage.total_tokens > 0, "expected non-zero total tokens");
    assert!(
        usage.total_tokens >= usage.input_tokens && usage.total_tokens >= usage.output_tokens,
        "expected total tokens to be >= input and output tokens"
    );
}

#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY environment variable"]
async fn test_openai_responses_stream_tool_call_ids_non_empty() {
    dotenv().ok();
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let client = steer_core::api::openai::OpenAIClient::with_mode(
        api_key,
        steer_core::api::openai::OpenAIMode::Responses,
    )
    .expect("openai client");

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

#[tokio::test]
async fn test_openai_provider_rejects_session_file_image_input() {
    let provider = steer_core::api::openai::OpenAIClient::with_mode(
        "test-key".to_string(),
        steer_core::api::openai::OpenAIMode::Responses,
    )
    .expect("openai client should build");

    let messages = vec![build_image_message(ImageSource::SessionFile {
        relative_path: "session-1/image.png".to_string(),
    })];

    let err = provider
        .complete(
            &builtin::gpt_5_nano_2025_08_07(),
            messages,
            None,
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .expect_err("session-file image should be rejected before request send");

    assert!(matches!(
        err,
        ApiError::UnsupportedFeature {
            provider,
            feature,
            ..
        } if provider == "openai" && feature == "image input source"
    ));
}

#[tokio::test]
async fn test_anthropic_provider_rejects_session_file_image_input() {
    let provider =
        steer_core::api::claude::AnthropicClient::new("test-key").expect("anthropic client");

    let messages = vec![build_image_message(ImageSource::SessionFile {
        relative_path: "session-1/image.png".to_string(),
    })];

    let err = provider
        .complete(
            &builtin::claude_haiku_4_5(),
            messages,
            None,
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .expect_err("session-file image should be rejected before request send");

    assert!(matches!(
        err,
        ApiError::UnsupportedFeature {
            provider,
            feature,
            ..
        } if provider == "anthropic" && feature == "image input source"
    ));
}

#[tokio::test]
async fn test_gemini_provider_rejects_session_file_image_input() {
    let provider = steer_core::api::gemini::GeminiClient::new("test-key");

    let messages = vec![build_image_message(ImageSource::SessionFile {
        relative_path: "session-1/image.png".to_string(),
    })];

    let err = provider
        .complete(
            &builtin::gemini_3_flash_preview(),
            messages,
            None,
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .expect_err("session-file image should be rejected before request send");

    assert!(matches!(
        err,
        ApiError::UnsupportedFeature {
            provider,
            feature,
            ..
        } if provider == "google" && feature == "image input source"
    ));
}

#[tokio::test]
async fn test_xai_provider_rejects_session_file_image_input() {
    let provider =
        steer_core::api::xai::XAIClient::new("test-key".to_string()).expect("xai client");

    let messages = vec![build_image_message(ImageSource::SessionFile {
        relative_path: "session-1/image.png".to_string(),
    })];

    let err = provider
        .complete(
            &builtin::grok_4_1_fast_reasoning(),
            messages,
            None,
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .expect_err("session-file image should be rejected before request send");

    assert!(matches!(
        err,
        ApiError::UnsupportedFeature {
            provider,
            feature,
            ..
        } if provider == "xai" && feature == "image input source"
    ));
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[tokio::test]
#[ignore = "requires external API credentials"]
async fn test_api_with_image_input(#[case] model: ModelId) {
    let client = test_client();
    run_api_with_data_url_image(&client, &model)
        .await
        .unwrap_or_else(|err| panic!("image input test failed for {model:?}: {err:?}"));
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore = "requires external API credentials"]
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
#[ignore = "requires external API credentials"]
async fn test_api_dispatch_agent_tool_call(#[case] model: ModelId) {
    let client = test_client();
    run_api_with_dispatch_agent_tool_call(&client, &model)
        .await
        .unwrap_or_else(|err| {
            panic!("dispatch_agent tool call test failed for {model:?}: {err:?}")
        });
}

#[tokio::test]
#[ignore = "requires external API credentials"]
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
#[ignore = "requires external API credentials"]
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
#[ignore = "requires external API credentials"]
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
#[ignore = "requires external API credentials"]
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
#[ignore = "requires external API credentials"]
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
#[ignore = "requires external API credentials"]
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
#[ignore = "requires external API credentials"]
async fn test_api_with_pruned_duplicate_read_file_history(#[case] model: ModelId) {
    let client = test_client();
    run_api_with_pruned_duplicate_read_file_history(&client, &model)
        .await
        .unwrap_or_else(|err| {
            panic!("pruned duplicate read_file history test failed for {model:?}: {err:?}")
        });
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore = "requires external API credentials"]
async fn test_api_with_pruned_pre_edit_read_history(#[case] model: ModelId) {
    let client = test_client();
    run_api_with_pruned_pre_edit_read_history(&client, &model)
        .await
        .unwrap_or_else(|err| {
            panic!("pruned pre-edit read history test failed for {model:?}: {err:?}")
        });
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore = "requires external API credentials"]
async fn test_api_with_pruned_assistant_text_and_tool_history(#[case] model: ModelId) {
    let client = test_client();
    run_api_with_pruned_assistant_text_and_tool_history(&client, &model)
        .await
        .unwrap_or_else(|err| {
            panic!("pruned assistant text/tool history test failed for {model:?}: {err:?}")
        });
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore = "requires external API credentials"]
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
#[ignore = "requires external API credentials"]
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
#[ignore = "requires external API credentials"]
async fn test_api_streaming_with_reasoning(#[case] model: ModelId) {
    let client = test_client();
    run_streaming_with_reasoning(&client, &model)
        .await
        .unwrap_or_else(|err| panic!("streaming reasoning test failed for {model:?}: {err:?}"));
}

#[tokio::test]
#[ignore = "requires external API credentials"]
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

async fn run_api_with_max_tokens(client: &Client, model: &ModelId) -> ApiTestResult<()> {
    let timestamp = Message::current_timestamp();
    let messages = vec![Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "Write a very long story about a dragon.".to_string(),
            }],
        },
        timestamp,
        id: Message::generate_id("user", timestamp),
        parent_message_id: None,
    }];

    let params = Some(steer_core::config::model::ModelParameters {
        max_output_tokens: Some(16),
        ..Default::default()
    });

    let mut stream = client
        .stream_complete(
            model,
            messages,
            None,
            None,
            params,
            CancellationToken::new(),
        )
        .await?;

    let mut total_text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::TextDelta(text) => {
                total_text.push_str(&text);
            }
            StreamChunk::MessageComplete(response) => {
                if let Some(usage) = response.usage {
                    assert!(
                        usage.output_tokens <= 20,
                        "expected output tokens to be limited by max_tokens, got {}",
                        usage.output_tokens
                    );
                }
                break;
            }
            StreamChunk::Error(e) => {
                return Err(ApiTestError::Api(ApiError::StreamError {
                    provider: model.provider.to_string(),
                    details: format!("{e:?}"),
                }));
            }
            _ => {}
        }
    }

    Ok(())
}

#[rstest]
#[case::claude_haiku_4_5(builtin::claude_haiku_4_5())]
#[case::gpt_5_nano_2025_08_07(builtin::gpt_5_nano_2025_08_07())]
#[case::gemini_3_flash_preview(builtin::gemini_3_flash_preview())]
#[case::grok_4_1_fast_reasoning(builtin::grok_4_1_fast_reasoning())]
#[tokio::test]
#[ignore = "requires external API credentials"]
async fn test_api_with_max_tokens(#[case] model: ModelId) {
    let client = test_client();
    run_api_with_max_tokens(&client, &model)
        .await
        .unwrap_or_else(|err| panic!("max_tokens test failed for {model:?}: {err:?}"));
}
