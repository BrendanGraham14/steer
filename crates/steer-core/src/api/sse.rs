use eventsource_stream::Eventsource;
use futures_core::Stream;
use futures_util::StreamExt;
use std::pin::Pin;
use tokio_util::bytes::Bytes;

use crate::api::error::ApiError;

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event_type: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

pub type SseStream = Pin<Box<dyn Stream<Item = Result<SseEvent, ApiError>> + Send>>;

pub fn parse_sse_stream<S, E>(byte_stream: S) -> SseStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + 'static,
{
    let event_stream = byte_stream
        .map(|result| result.map_err(|e| std::io::Error::other(e.to_string())))
        .eventsource()
        .map(|result| {
            result
                .map(|event| SseEvent {
                    event_type: if event.event.is_empty() {
                        None
                    } else {
                        Some(event.event)
                    },
                    data: event.data,
                    id: if event.id.is_empty() {
                        None
                    } else {
                        Some(event.id)
                    },
                })
                .map_err(|e| ApiError::StreamError {
                    provider: "sse".to_string(),
                    details: e.to_string(),
                })
        });

    Box::pin(event_stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    #[tokio::test]
    async fn test_parse_simple_sse_event() {
        let sse_data = "event: message\ndata: {\"text\": \"hello\"}\n\n";
        let byte_stream =
            stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(sse_data)) });

        let mut sse_stream = parse_sse_stream(byte_stream);

        let event = sse_stream.next().await.unwrap().unwrap();
        assert_eq!(event.event_type, Some("message".to_string()));
        assert_eq!(event.data, "{\"text\": \"hello\"}");
    }

    #[tokio::test]
    async fn test_parse_multiple_sse_events() {
        let sse_data = "event: start\ndata: first\n\nevent: delta\ndata: second\n\n";
        let byte_stream =
            stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(sse_data)) });

        let mut sse_stream = parse_sse_stream(byte_stream);

        let event1 = sse_stream.next().await.unwrap().unwrap();
        assert_eq!(event1.event_type, Some("start".to_string()));
        assert_eq!(event1.data, "first");

        let event2 = sse_stream.next().await.unwrap().unwrap();
        assert_eq!(event2.event_type, Some("delta".to_string()));
        assert_eq!(event2.data, "second");
    }
}
