use super::conversions::*;
use steer_core::session::state::{
    BackendConfig, RemoteAuth, SessionToolConfig, ToolApprovalPolicy, ToolFilter, ToolVisibility,
    WorkspaceConfig,
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
        approval_variant in 0..2usize,
        pre_approved_tools in prop::collection::vec("[a-z]+", 0..5),
        metadata_key in "[a-z]+",
        metadata_value in "[a-z0-9]+",
    ) -> SessionToolConfig {
        let mut metadata = HashMap::new();
        metadata.insert(metadata_key, metadata_value);
        let approval_policy = match approval_variant {
            0 => ToolApprovalPolicy::AlwaysAsk,
            _ => ToolApprovalPolicy::PreApproved {
                tools: pre_approved_tools.into_iter().collect(),
            },
        };
        SessionToolConfig {
            backends,
            visibility,
            approval_policy,
            tools: HashMap::new(),
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
    use crate::client_api::{ClientEvent, OpId, RequestId};
    use crate::grpc::conversions::{proto_to_client_event, session_event_to_proto};
    use steer_core::app::domain::event::{CancellationInfo, SessionEvent};
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
            ClientEvent::ProcessingStarted { op_id: received } => {
                assert_eq!(op_id, received);
            }
            other => panic!("Expected ProcessingStarted, got {:?}", other),
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
            other => panic!("Expected ProcessingCompleted, got {:?}", other),
        }
    }

    #[test]
    fn test_op_id_preserved_in_operation_cancelled() {
        let op_id = OpId::from(Uuid::new_v4());
        let event = SessionEvent::OperationCancelled {
            op_id,
            info: CancellationInfo {
                pending_tool_calls: 3,
            },
        };

        let proto_response = session_event_to_proto(event, 1).unwrap();
        let client_event = proto_to_client_event(proto_response).unwrap().unwrap();

        match client_event {
            ClientEvent::OperationCancelled {
                op_id: received,
                pending_tool_calls,
            } => {
                assert_eq!(op_id, received);
                assert_eq!(pending_tool_calls, 0);
            }
            other => panic!("Expected OperationCancelled, got {:?}", other),
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
            other => panic!("Expected ApprovalRequested, got {:?}", other),
        }
    }
}

#[cfg(test)]
mod event_conversion_tests {
    use crate::client_api::{ClientEvent, MessageId, OpId, ToolCallDelta, ToolCallId};
    use crate::grpc::conversions::{proto_to_client_event, session_event_to_proto, stream_delta_to_proto};
    use steer_core::app::domain::delta::StreamDelta;
    use steer_core::app::domain::event::{CompactResult, SessionEvent};
    use uuid::Uuid;

    #[test]
    fn test_compact_result_event_roundtrip() {
        let event = SessionEvent::CompactResult {
            result: CompactResult::Success("summary".to_string()),
        };

        let proto = session_event_to_proto(event, 42).unwrap();
        let client_event = proto_to_client_event(proto).unwrap().unwrap();

        match client_event {
            ClientEvent::CompactResult { result } => {
                assert!(matches!(result, CompactResult::Success(ref s) if s == "summary"));
            }
            other => panic!("Expected CompactResult, got {:?}", other),
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

        let proto = stream_delta_to_proto(thinking_delta, 0).unwrap();
        let client_event = proto_to_client_event(proto).unwrap().unwrap();
        match client_event {
            ClientEvent::ThinkingDelta { op_id: received, message_id: msg, delta } => {
                assert_eq!(received, op_id);
                assert_eq!(msg, message_id);
                assert_eq!(delta, "thinking...");
            }
            other => panic!("Expected ThinkingDelta, got {:?}", other),
        }

        let tool_call_id = ToolCallId::from_string("tool_1");
        let tool_delta = StreamDelta::ToolCallChunk {
            op_id,
            message_id: message_id.clone(),
            tool_call_id: tool_call_id.clone(),
            delta: steer_core::app::domain::delta::ToolCallDelta::ArgumentChunk("{\"x\":".to_string()),
        };

        let proto = stream_delta_to_proto(tool_delta, 0).unwrap();
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
            other => panic!("Expected ToolCallDelta, got {:?}", other),
        }
    }
}
