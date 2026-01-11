use crate::api::Provider;
use crate::api::error::StreamError;
use crate::api::provider::StreamChunk;
use crate::api::sse::SseEvent;
use crate::api::xai::XAIClient;
use crate::app::conversation::{AssistantContent, MessageData, UserContent};
use futures::StreamExt;
use steer_tools::ToolSchema;
use tokio_util::sync::CancellationToken;

#[test]
fn test_xai_client_creation() {
    let api_key = "test_key".to_string();
    let client = XAIClient::new(api_key);
    assert_eq!(client.name(), "xai");
}

#[tokio::test]
async fn test_xai_client_provider_trait() {
    let api_key = "test_key".to_string();
    let client = XAIClient::new(api_key);

    let _name: &str = client.name();
    assert_eq!(_name, "xai");

    let messages = vec![];
    let model_id = crate::config::model::ModelId::new(
        crate::config::provider::xai(),
        "grok-3-mini",
    );
    let result = client
        .complete(
            &model_id,
            messages,
            None,
            None::<Vec<ToolSchema>>,
            None,
            CancellationToken::new(),
        )
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_convert_xai_stream_text_deltas() {
    use futures::stream;
    use std::pin::pin;

    let events = vec![
        Ok(SseEvent {
            event_type: None,
            data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#.to_string(),
            id: None,
        }),
        Ok(SseEvent {
            event_type: None,
            data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}"#.to_string(),
            id: None,
        }),
        Ok(SseEvent {
            event_type: None,
            data: "[DONE]".to_string(),
            id: None,
        }),
    ];

    let sse_stream = stream::iter(events);
    let token = CancellationToken::new();
    let mut stream = pin!(XAIClient::convert_xai_stream(sse_stream, token));

    let first_delta = stream.next().await.unwrap();
    assert!(matches!(first_delta, StreamChunk::TextDelta(ref t) if t == "Hello"));

    let second_delta = stream.next().await.unwrap();
    assert!(matches!(second_delta, StreamChunk::TextDelta(ref t) if t == " world"));

    let complete = stream.next().await.unwrap();
    assert!(matches!(complete, StreamChunk::MessageComplete(_)));
}

#[tokio::test]
async fn test_convert_xai_stream_with_tool_calls() {
    use futures::stream;
    use std::pin::pin;

    let events = vec![
        Ok(SseEvent {
            event_type: None,
            data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"search","arguments":""}}]},"finish_reason":null}]}"#.to_string(),
            id: None,
        }),
        Ok(SseEvent {
            event_type: None,
            data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"query\":"}}]},"finish_reason":null}]}"#.to_string(),
            id: None,
        }),
        Ok(SseEvent {
            event_type: None,
            data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"test\"}"}}]},"finish_reason":null}]}"#.to_string(),
            id: None,
        }),
        Ok(SseEvent {
            event_type: None,
            data: "[DONE]".to_string(),
            id: None,
        }),
    ];

    let sse_stream = stream::iter(events);
    let token = CancellationToken::new();
    let mut stream = pin!(XAIClient::convert_xai_stream(sse_stream, token));

    let tool_start = stream.next().await.unwrap();
    assert!(
        matches!(tool_start, StreamChunk::ToolUseStart { ref id, ref name } if id == "call_abc" && name == "search")
    );

    let arg_delta_1 = stream.next().await.unwrap();
    assert!(
        matches!(arg_delta_1, StreamChunk::ToolUseInputDelta { ref delta, .. } if delta == "{\"query\":")
    );

    let arg_delta_2 = stream.next().await.unwrap();
    assert!(
        matches!(arg_delta_2, StreamChunk::ToolUseInputDelta { ref delta, .. } if delta == "\"test\"}")
    );

    let complete = stream.next().await.unwrap();
    if let StreamChunk::MessageComplete(response) = complete {
        assert_eq!(response.content.len(), 1);
        assert!(matches!(
            &response.content[0],
            AssistantContent::ToolCall { .. }
        ));
    } else {
        panic!("Expected MessageComplete");
    }
}

#[tokio::test]
async fn test_convert_xai_stream_cancellation() {
    use futures::stream;

    let events = vec![Ok(SseEvent {
        event_type: None,
        data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#.to_string(),
        id: None,
    })];

    let sse_stream = stream::iter(events);
    let token = CancellationToken::new();
    token.cancel();

    let mut stream = XAIClient::convert_xai_stream(sse_stream, token);

    let cancelled = stream.next().await.unwrap();
    assert!(matches!(
        cancelled,
        StreamChunk::Error(StreamError::Cancelled)
    ));
}

#[tokio::test]
#[ignore = "Requires XAI_API_KEY environment variable"]
async fn test_stream_complete_real_api() {
    dotenvy::dotenv().ok();
    let api_key = std::env::var("XAI_API_KEY").expect("XAI_API_KEY must be set");
    let client = XAIClient::new(api_key);

    let message = crate::app::conversation::Message {
        data: MessageData::User {
            content: vec![UserContent::Text {
                text: "Say exactly: Hello".to_string(),
            }],
        },
        timestamp: chrono::Utc::now().timestamp_millis() as u64,
        id: "test-msg".to_string(),
        parent_message_id: None,
    };

    let model_id = crate::config::model::ModelId::new(
        crate::config::provider::xai(),
        "grok-3-mini",
    );
    let token = CancellationToken::new();

    let mut stream = client
        .stream_complete(&model_id, vec![message], None, None, None, token)
        .await
        .expect("stream_complete should succeed");

    let mut got_text_delta = false;
    let mut got_message_complete = false;
    let mut accumulated_text = String::new();

    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::TextDelta(text) => {
                got_text_delta = true;
                accumulated_text.push_str(&text);
            }
            StreamChunk::MessageComplete(response) => {
                got_message_complete = true;
                assert!(!response.content.is_empty());
            }
            StreamChunk::Error(e) => panic!("Unexpected error: {:?}", e),
            _ => {}
        }
    }

    assert!(got_text_delta, "Should receive at least one TextDelta");
    assert!(
        got_message_complete,
        "Should receive MessageComplete at the end"
    );
    assert!(
        accumulated_text.to_lowercase().contains("hello"),
        "Response should contain 'hello', got: {}",
        accumulated_text
    );
}
