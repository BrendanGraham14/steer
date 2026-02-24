use super::conversions::*;
use steer_core::session::state::{
    ApprovalRules, BackendConfig, RemoteAuth, SessionToolConfig, ToolApprovalPolicy, ToolFilter,
    ToolVisibility, UnapprovedBehavior, WorkspaceConfig,
};
use steer_core::tools::McpTransport;

#[cfg(test)]
use proptest::prelude::*;
use std::collections::HashMap;

prop_compose! {
    fn arb_tool_filter()(
        variant in 0..3usize,
        tools in prop::collection::vec("[a-z]+", 0..5)
    ) -> ToolFilter {
        match variant {
            0 => ToolFilter::All,
            1 => ToolFilter::Include(tools),
            _ => ToolFilter::Exclude(tools),
        }
    }
}

prop_compose! {
    fn arb_remote_auth()(
        variant in 0..2usize,
        token in "[a-zA-Z0-9]+",
    ) -> RemoteAuth {
        match variant {
            0 => RemoteAuth::Bearer { token },
            _ => RemoteAuth::ApiKey { key: token },
        }
    }
}

prop_compose! {
    fn arb_workspace_config()(
        variant in 0..2usize,
        path in "[a-z/]+",
        agent_address in "http://[a-z]+\\.example\\.com",
        auth in arb_remote_auth(),
    ) -> WorkspaceConfig {
        match variant {
            0 => WorkspaceConfig::Local { path: path.into() },
            _ => WorkspaceConfig::Remote { agent_address, auth: Some(auth) },
        }
    }
}

prop_compose! {
    fn arb_mcp_transport()(
        variant in 0..4usize,
        command in "[a-z]+",
        args in prop::collection::vec("[a-z]+", 0..3),
        host in "[a-z]+\\.local",
        port in 1024u16..65535,
        url in "http://localhost:[0-9]+",
    ) -> McpTransport {
        match variant {
            0 => McpTransport::Stdio { command, args },
            1 => McpTransport::Tcp { host, port },
            2 => McpTransport::Sse { url: url.clone(), headers: None },
            _ => McpTransport::Http { url, headers: None },
        }
    }
}

prop_compose! {
    fn arb_backend_config()(
        server_name in "[a-z]+",
        transport in arb_mcp_transport(),
        tool_filter in arb_tool_filter(),
    ) -> BackendConfig {
        BackendConfig::Mcp {
            server_name,
            transport,
            tool_filter,
        }
    }
}

prop_compose! {
    fn arb_session_tool_config()(
        backends in prop::collection::vec(arb_backend_config(), 0..3),
        visibility in prop::sample::select(vec![ToolVisibility::All, ToolVisibility::ReadOnly]),
        default_behavior in prop::sample::select(vec![
            UnapprovedBehavior::Prompt,
            UnapprovedBehavior::Deny,
            UnapprovedBehavior::Allow,
        ]),
        pre_approved_tools in prop::collection::vec("[a-z]+", 0..5),
        metadata_key in "[a-z]+",
        metadata_value in "[a-z0-9]+",
    ) -> SessionToolConfig {
        let mut metadata = HashMap::new();
        metadata.insert(metadata_key, metadata_value);
        let approval_policy = ToolApprovalPolicy {
            default_behavior,
            preapproved: ApprovalRules {
                tools: pre_approved_tools.into_iter().collect(),
                per_tool: HashMap::new(),
            },
        };
        SessionToolConfig {
            backends,
            visibility,
            approval_policy,
            metadata,
        }
    }
}

proptest! {
    #[test]
    fn prop_tool_filter_roundtrip(filter in arb_tool_filter()) {
        let proto = tool_filter_to_proto(&filter);
        let roundtrip = proto_to_tool_filter(Some(proto));
        prop_assert_eq!(filter, roundtrip);
    }

    #[test]
    fn prop_workspace_config_roundtrip(config in arb_workspace_config()) {
        let proto = workspace_config_to_proto(&config);
        let roundtrip = proto_to_workspace_config(proto);
        prop_assert_eq!(config, roundtrip);
    }

    #[test]
    fn prop_session_tool_config_roundtrip(config in arb_session_tool_config()) {
        let proto = session_tool_config_to_proto(&config);
        let roundtrip = proto_to_tool_config(proto);

        prop_assert_eq!(config.visibility, roundtrip.visibility);
        prop_assert_eq!(config.approval_policy, roundtrip.approval_policy);

        prop_assert_eq!(config.backends.len(), roundtrip.backends.len());
        for (b1, b2) in config.backends.iter().zip(roundtrip.backends.iter()) {
            match (b1, b2) {
                (BackendConfig::Mcp { server_name: sn1, transport: t1, tool_filter: tf1 },
                 BackendConfig::Mcp { server_name: sn2, transport: t2, tool_filter: tf2 }) => {
                    prop_assert_eq!(sn1, sn2);
                    match (t1, t2) {
                        (McpTransport::Stdio { command: c1, args: a1 },
                         McpTransport::Stdio { command: c2, args: a2 }) => {
                            prop_assert_eq!(c1, c2);
                            prop_assert_eq!(a1, a2);
                        }
                        (McpTransport::Tcp { host: h1, port: p1 },
                         McpTransport::Tcp { host: h2, port: p2 }) => {
                            prop_assert_eq!(h1, h2);
                            prop_assert_eq!(p1, p2);
                        }
                        _ => {}
                    }
                    prop_assert_eq!(tf1, tf2);
                }
            }
        }

        prop_assert_eq!(config.metadata, roundtrip.metadata);
    }
}

#[cfg(test)]
mod id_preservation_tests {
    use crate::client_api::{ClientEvent, OpId, QueuedWorkKind, RequestId};
    use crate::grpc::conversions::{proto_to_client_event, session_event_to_proto};
    use steer_core::app::domain::event::{CancellationInfo, CompactTrigger, SessionEvent};
    use steer_core::app::domain::state::OperationKind;
    use steer_tools::ToolCall;
    use uuid::Uuid;

    #[test]
    fn test_op_id_preserved_in_operation_started() {
        let op_id = OpId::from(Uuid::new_v4());
        let event = SessionEvent::OperationStarted {
            op_id,
            kind: OperationKind::AgentLoop,
        };

        let proto_response = session_event_to_proto(event, 1).unwrap();
        let client_event = proto_to_client_event(proto_response).unwrap().unwrap();

        match client_event {
            ClientEvent::ProcessingStarted {
                op_id: received,
                operation_kind,
            } => {
                assert_eq!(op_id, received);
                assert!(matches!(operation_kind, Some(OperationKind::AgentLoop)));
            }
            other => panic!("Expected ProcessingStarted, got {other:?}"),
        }
    }

    #[test]
    fn test_operation_kind_preserved_for_auto_compact() {
        let op_id = OpId::from(Uuid::new_v4());
        let event = SessionEvent::OperationStarted {
            op_id,
            kind: OperationKind::Compact {
                trigger: CompactTrigger::Auto,
            },
        };

        let proto_response = session_event_to_proto(event, 1).unwrap();
        let client_event = proto_to_client_event(proto_response).unwrap().unwrap();

        match client_event {
            ClientEvent::ProcessingStarted {
                op_id: received,
                operation_kind,
            } => {
                assert_eq!(op_id, received);
                assert!(matches!(
                    operation_kind,
                    Some(OperationKind::Compact {
                        trigger: CompactTrigger::Auto
                    })
                ));
            }
            other => panic!("Expected ProcessingStarted, got {other:?}"),
        }
    }

    #[test]
    fn test_operation_kind_preserved_for_direct_bash() {
        let op_id = OpId::from(Uuid::new_v4());
        let event = SessionEvent::OperationStarted {
            op_id,
            kind: OperationKind::DirectBash {
                command: "ls -la".to_string(),
            },
        };

        let proto_response = session_event_to_proto(event, 1).unwrap();
        let client_event = proto_to_client_event(proto_response).unwrap().unwrap();

        match client_event {
            ClientEvent::ProcessingStarted {
                op_id: received,
                operation_kind,
            } => {
                assert_eq!(op_id, received);
                assert!(matches!(
                    operation_kind,
                    Some(OperationKind::DirectBash { command }) if command == "ls -la"
                ));
            }
            other => panic!("Expected ProcessingStarted, got {other:?}"),
        }
    }

    #[test]
    fn test_op_id_preserved_in_operation_completed() {
        let op_id = OpId::from(Uuid::new_v4());
        let event = SessionEvent::OperationCompleted { op_id };

        let proto_response = session_event_to_proto(event, 1).unwrap();
        let client_event = proto_to_client_event(proto_response).unwrap().unwrap();

        match client_event {
            ClientEvent::ProcessingCompleted { op_id: received } => {
                assert_eq!(op_id, received);
            }
            other => panic!("Expected ProcessingCompleted, got {other:?}"),
        }
    }

    #[test]
    fn test_op_id_preserved_in_operation_cancelled() {
        let op_id = OpId::from(Uuid::new_v4());
        let event = SessionEvent::OperationCancelled {
            op_id,
            info: CancellationInfo {
                pending_tool_calls: 3,
                popped_queued_item: Some(steer_core::app::domain::event::QueuedWorkItemSnapshot {
                    kind: Some(steer_core::app::domain::event::QueuedWorkKind::UserMessage),
                    content: "queued text".to_string(),
                    queued_at: 123,
                    model: None,
                    op_id,
                    message_id: steer_core::app::domain::types::MessageId::from_string(
                        "msg_queued",
                    ),
                    attachment_count: 0,
                }),
            },
        };

        let proto_response = session_event_to_proto(event, 1).unwrap();
        let client_event = proto_to_client_event(proto_response).unwrap().unwrap();

        match client_event {
            ClientEvent::OperationCancelled {
                op_id: received,
                pending_tool_calls,
                popped_queued_item,
            } => {
                assert_eq!(op_id, received);
                assert_eq!(pending_tool_calls, 0);
                let popped = popped_queued_item.expect("expected popped queued item");
                assert_eq!(popped.kind, QueuedWorkKind::UserMessage);
                assert_eq!(popped.content, "queued text");
            }
            other => panic!("Expected OperationCancelled, got {other:?}"),
        }
    }

    #[test]
    fn test_request_id_preserved_in_approval_requested() {
        let request_id = RequestId::from(Uuid::new_v4());
        let tool_call = ToolCall {
            id: "tool_123".to_string(),
            name: "bash".to_string(),
            parameters: serde_json::json!({"command": "ls"}),
        };
        let event = SessionEvent::ApprovalRequested {
            request_id,
            tool_call: tool_call.clone(),
        };

        let proto_response = session_event_to_proto(event, 1).unwrap();
        let client_event = proto_to_client_event(proto_response).unwrap().unwrap();

        match client_event {
            ClientEvent::ApprovalRequested {
                request_id: received,
                tool_call: received_tool,
            } => {
                assert_eq!(request_id, received);
                assert_eq!(tool_call.name, received_tool.name);
                assert_eq!(tool_call.parameters, received_tool.parameters);
            }
            other => panic!("Expected ApprovalRequested, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod event_conversion_tests {
    use crate::client_api::{
        ClientEvent, ContextWindowUsage, MessageId, OpId, ToolCallDelta, ToolCallId,
        UsageUpdateKind,
    };
    use crate::grpc::conversions::{
        message_to_proto, proto_to_client_event, session_event_to_proto, stream_delta_to_proto,
    };
    use steer_core::api::provider::TokenUsage;
    use steer_core::app::conversation::{
        AssistantContent, ImageContent, ImageSource, Message, MessageData, UserContent,
    };
    use steer_core::app::domain::delta::StreamDelta;
    use steer_core::app::domain::event::{CompactResult, CompactTrigger, SessionEvent};
    use steer_core::config::model::builtin;
    use steer_proto::agent::v1 as proto;
    use uuid::Uuid;

    #[test]
    fn test_compact_result_event_roundtrip() {
        let event = SessionEvent::CompactResult {
            result: CompactResult::Success("summary".to_string()),
            trigger: CompactTrigger::Manual,
        };

        let proto = session_event_to_proto(event, 42).unwrap();
        let client_event = proto_to_client_event(proto).unwrap().unwrap();

        match client_event {
            ClientEvent::CompactResult { result, trigger } => {
                assert!(matches!(result, CompactResult::Success(ref s) if s == "summary"));
                assert_eq!(trigger, CompactTrigger::Manual);
            }
            other => panic!("Expected CompactResult, got {other:?}"),
        }
    }

    #[test]
    fn test_compact_result_auto_trigger_roundtrip() {
        let event = SessionEvent::CompactResult {
            result: CompactResult::Cancelled,
            trigger: CompactTrigger::Auto,
        };

        let proto = session_event_to_proto(event, 42).unwrap();
        let client_event = proto_to_client_event(proto).unwrap().unwrap();

        match client_event {
            ClientEvent::CompactResult { result, trigger } => {
                assert!(matches!(result, CompactResult::Cancelled));
                assert_eq!(trigger, CompactTrigger::Auto);
            }
            other => panic!("Expected CompactResult, got {other:?}"),
        }
    }

    #[test]
    fn test_compact_result_failed_roundtrip() {
        let event = SessionEvent::CompactResult {
            result: CompactResult::Failed("model error".to_string()),
            trigger: CompactTrigger::Manual,
        };

        let proto = session_event_to_proto(event, 42).unwrap();
        let client_event = proto_to_client_event(proto).unwrap().unwrap();

        match client_event {
            ClientEvent::CompactResult { result, trigger } => {
                assert!(matches!(result, CompactResult::Failed(ref s) if s == "model error"));
                assert_eq!(trigger, CompactTrigger::Manual);
            }
            other => panic!("Expected CompactResult, got {other:?}"),
        }
    }

    #[test]
    fn test_llm_usage_event_roundtrip() {
        let op_id = OpId::from(Uuid::new_v4());
        let model = builtin::claude_sonnet_4_5();
        let usage = TokenUsage {
            input_tokens: 123,
            output_tokens: 45,
            total_tokens: 168,
        };
        let context_window = Some(ContextWindowUsage {
            max_context_tokens: Some(200_000),
            remaining_tokens: Some(199_832),
            utilization_ratio: Some(0.00084),
            estimated: true,
        });

        let event = SessionEvent::LlmUsageUpdated {
            op_id,
            model: model.clone(),
            usage,
            context_window: context_window.clone(),
        };

        let proto = session_event_to_proto(event, 99).unwrap();
        let client_event = proto_to_client_event(proto).unwrap().unwrap();

        match client_event {
            ClientEvent::LlmUsageUpdated {
                op_id: received_op_id,
                model: received_model,
                usage: received_usage,
                context_window: received_context,
                kind,
            } => {
                assert_eq!(received_op_id, op_id);
                assert_eq!(received_model, model);
                assert_eq!(received_usage, usage);
                assert_eq!(received_context, context_window);
                assert_eq!(kind, UsageUpdateKind::Final);
            }
            other => panic!("Expected LlmUsageUpdated, got {other:?}"),
        }
    }

    #[test]
    fn test_llm_usage_event_known_kind_values_map_exhaustively() {
        let op_id = Uuid::new_v4().to_string();
        let usage = steer_proto::agent::v1::Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            cost_usd: None,
        };

        let cases = [
            (
                steer_proto::agent::v1::UsageUpdateKind::Unspecified as i32,
                UsageUpdateKind::Unspecified,
            ),
            (
                steer_proto::agent::v1::UsageUpdateKind::Partial as i32,
                UsageUpdateKind::Partial,
            ),
            (
                steer_proto::agent::v1::UsageUpdateKind::Final as i32,
                UsageUpdateKind::Final,
            ),
        ];

        for (kind_value, expected_kind) in cases {
            let proto = steer_proto::agent::v1::SessionEvent {
                sequence_num: 7,
                timestamp: None,
                event: Some(
                    steer_proto::agent::v1::session_event::Event::LlmUsageUpdated(
                        steer_proto::agent::v1::LlmUsageUpdatedEvent {
                            op_id: op_id.clone(),
                            model: Some(steer_proto::agent::v1::ModelSpec {
                                provider_id: "anthropic".to_string(),
                                model_id: "claude-sonnet-4-5".to_string(),
                            }),
                            usage: Some(usage.clone()),
                            context_window: None,
                            kind: kind_value,
                        },
                    ),
                ),
            };

            let event = proto_to_client_event(proto)
                .expect("known kind values should convert")
                .expect("session event should not be filtered");
            match event {
                ClientEvent::LlmUsageUpdated { kind, .. } => assert_eq!(kind, expected_kind),
                other => panic!("Expected LlmUsageUpdated, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_llm_usage_event_unknown_kind_rejected() {
        let proto = steer_proto::agent::v1::SessionEvent {
            sequence_num: 7,
            timestamp: None,
            event: Some(
                steer_proto::agent::v1::session_event::Event::LlmUsageUpdated(
                    steer_proto::agent::v1::LlmUsageUpdatedEvent {
                        op_id: Uuid::new_v4().to_string(),
                        model: Some(steer_proto::agent::v1::ModelSpec {
                            provider_id: "anthropic".to_string(),
                            model_id: "claude-sonnet-4-5".to_string(),
                        }),
                        usage: Some(steer_proto::agent::v1::Usage {
                            input_tokens: 10,
                            output_tokens: 5,
                            total_tokens: 15,
                            cost_usd: None,
                        }),
                        context_window: None,
                        kind: 999,
                    },
                ),
            ),
        };

        let err = proto_to_client_event(proto).expect_err("expected invalid enum error");
        assert!(matches!(
            err,
            crate::grpc::error::ConversionError::InvalidEnumValue {
                value: 999,
                enum_name,
            } if enum_name == "UsageUpdateKind"
        ));
    }

    #[test]
    fn test_message_to_proto_preserves_image_content() {
        let user_message = Message {
            timestamp: 1,
            id: "user-img-1".to_string(),
            parent_message_id: None,
            data: MessageData::User {
                content: vec![UserContent::Image {
                    image: ImageContent {
                        mime_type: "image/png".to_string(),
                        source: ImageSource::SessionFile {
                            relative_path: "session-id/hash.png".to_string(),
                        },
                        width: Some(120),
                        height: Some(80),
                        bytes: Some(1024),
                        sha256: Some("abc123".to_string()),
                    },
                }],
            },
        };

        let user_proto = message_to_proto(user_message).unwrap();
        let user_variant = user_proto.message.expect("message");
        let user_block = match user_variant {
            steer_proto::agent::v1::message::Message::User(message) => {
                message.content.into_iter().next().expect("content block")
            }
            other => panic!("Expected user message, got {other:?}"),
        };
        match user_block.content {
            Some(steer_proto::agent::v1::user_content::Content::Image(image)) => {
                assert_eq!(image.mime_type, "image/png");
                assert_eq!(image.width, Some(120));
                assert_eq!(image.height, Some(80));
                assert_eq!(image.bytes, Some(1024));
                assert_eq!(image.sha256.as_deref(), Some("abc123"));
                match image.source {
                    Some(steer_proto::agent::v1::image_content::Source::SessionFile(source)) => {
                        assert_eq!(source.relative_path, "session-id/hash.png");
                    }
                    other => panic!("Expected session file source, got {other:?}"),
                }
            }
            other => panic!("Expected image content block, got {other:?}"),
        }

        let assistant_message = Message {
            timestamp: 2,
            id: "assistant-img-1".to_string(),
            parent_message_id: Some("user-img-1".to_string()),
            data: MessageData::Assistant {
                content: vec![AssistantContent::Image {
                    image: ImageContent {
                        mime_type: "image/jpeg".to_string(),
                        source: ImageSource::DataUrl {
                            data_url: "data:image/jpeg;base64,Zm9v".to_string(),
                        },
                        width: None,
                        height: None,
                        bytes: None,
                        sha256: None,
                    },
                }],
            },
        };

        let assistant_proto = message_to_proto(assistant_message).unwrap();
        let assistant_variant = assistant_proto.message.expect("message");
        let assistant_block = match assistant_variant {
            steer_proto::agent::v1::message::Message::Assistant(message) => {
                message.content.into_iter().next().expect("content block")
            }
            other => panic!("Expected assistant message, got {other:?}"),
        };
        match assistant_block.content {
            Some(steer_proto::agent::v1::assistant_content::Content::Image(image)) => {
                assert_eq!(image.mime_type, "image/jpeg");
                match image.source {
                    Some(steer_proto::agent::v1::image_content::Source::DataUrl(source)) => {
                        assert_eq!(source.data_url, "data:image/jpeg;base64,Zm9v");
                    }
                    other => panic!("Expected data url source, got {other:?}"),
                }
            }
            other => panic!("Expected image content block, got {other:?}"),
        }
    }

    #[test]
    fn test_session_event_to_proto_preserves_user_image_content() {
        let event = SessionEvent::UserMessageAdded {
            message: Message {
                timestamp: 123,
                id: "msg-img".to_string(),
                parent_message_id: None,
                data: MessageData::User {
                    content: vec![UserContent::Image {
                        image: ImageContent {
                            mime_type: "image/webp".to_string(),
                            source: ImageSource::Url {
                                url: "https://example.com/image.webp".to_string(),
                            },
                            width: Some(64),
                            height: Some(64),
                            bytes: None,
                            sha256: None,
                        },
                    }],
                },
            },
        };

        let proto = session_event_to_proto(event, 7).unwrap();
        let event_variant = proto.event.expect("event");
        let user_message = match event_variant {
            steer_proto::agent::v1::session_event::Event::UserMessageAdded(event) => {
                event.message.expect("message")
            }
            other => panic!("Expected user message added event, got {other:?}"),
        };

        let content_block = user_message
            .content
            .into_iter()
            .next()
            .expect("content block");
        match content_block.content {
            Some(steer_proto::agent::v1::user_content::Content::Image(image)) => {
                assert_eq!(image.mime_type, "image/webp");
                assert_eq!(image.width, Some(64));
                assert_eq!(image.height, Some(64));
                match image.source {
                    Some(steer_proto::agent::v1::image_content::Source::Url(source)) => {
                        assert_eq!(source.url, "https://example.com/image.webp");
                    }
                    other => panic!("Expected url source, got {other:?}"),
                }
            }
            other => panic!("Expected image content block, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_delta_thinking_and_tool_call_roundtrip() {
        let op_id = OpId::from(Uuid::new_v4());
        let message_id = MessageId::from_string("msg_1");

        let thinking_delta = StreamDelta::ThinkingChunk {
            op_id,
            message_id: message_id.clone(),
            delta: "thinking...".to_string(),
        };

        let proto = stream_delta_to_proto(thinking_delta, 0, 0).unwrap();
        let client_event = proto_to_client_event(proto).unwrap().unwrap();
        match client_event {
            ClientEvent::ThinkingDelta {
                op_id: received,
                message_id: msg,
                delta,
            } => {
                assert_eq!(received, op_id);
                assert_eq!(msg, message_id);
                assert_eq!(delta, "thinking...");
            }
            other => panic!("Expected ThinkingDelta, got {other:?}"),
        }

        let tool_call_id = ToolCallId::from_string("tool_1");
        let tool_delta = StreamDelta::ToolCallChunk {
            op_id,
            message_id: message_id.clone(),
            tool_call_id: tool_call_id.clone(),
            delta: steer_core::app::domain::delta::ToolCallDelta::ArgumentChunk(
                "{\"x\":".to_string(),
            ),
        };

        let proto = stream_delta_to_proto(tool_delta, 0, 0).unwrap();
        let client_event = proto_to_client_event(proto).unwrap().unwrap();
        match client_event {
            ClientEvent::ToolCallDelta {
                op_id: received,
                message_id: msg,
                tool_call_id: received_tool,
                delta: ToolCallDelta::ArgumentChunk(chunk),
            } => {
                assert_eq!(received, op_id);
                assert_eq!(msg, message_id);
                assert_eq!(received_tool, tool_call_id);
                assert_eq!(chunk, "{\"x\":");
            }
            other => panic!("Expected ToolCallDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_reset_roundtrip() {
        let op_id = OpId::from(Uuid::new_v4());
        let message_id = MessageId::from_string("msg_reset");

        let reset_delta = StreamDelta::Reset {
            op_id,
            message_id: message_id.clone(),
        };

        let proto_event = stream_delta_to_proto(reset_delta, 0, 0).unwrap();
        let event = proto_event
            .event
            .as_ref()
            .expect("session event should be present");
        let proto::session_event::Event::StreamDelta(stream_delta) = event else {
            panic!("expected stream delta event")
        };
        assert!(matches!(
            stream_delta.delta_type,
            Some(proto::stream_delta_event::DeltaType::Reset(_))
        ));

        let client_event = proto_to_client_event(proto_event).unwrap().unwrap();
        match client_event {
            ClientEvent::StreamReset {
                op_id: received_op,
                message_id: received_msg,
            } => {
                assert_eq!(received_op, op_id);
                assert_eq!(received_msg, message_id);
            }
            other => panic!("Expected StreamReset, got {other:?}"),
        }
    }
}
