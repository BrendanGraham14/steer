//! Minimal round-trip tests for conversion helpers that have
//! *both* directions implemented.

use super::conversions::*;
use conductor_core::api::ToolCall;
use conductor_core::app::conversation::{
    AppCommandType, AssistantContent, CommandResponse,
    Message as ConversationMessage, ThoughtContent, ToolResult, UserContent,
};
use conductor_core::app::{
    AppEvent,
    cancellation::{ActiveTool, CancellationInfo},
};
use conductor_core::session::state::{
    BackendConfig, ContainerRuntime, RemoteAuth, SessionToolConfig, ToolApprovalPolicy, ToolFilter,
    ToolVisibility, WorkspaceConfig,
};
use serde_json::json;
use std::collections::HashMap;

#[cfg(test)]
use proptest::prelude::*;

// Property test strategies
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
        variant in 0..3usize,
        address in "[a-z]+://[a-z]+:[0-9]+",
        auth in proptest::option::of(arb_remote_auth()),
        image in "[a-z]+:[a-z]+",
        runtime in prop::sample::select(vec![ContainerRuntime::Docker, ContainerRuntime::Podman])
    ) -> WorkspaceConfig {
        match variant {
            0 => WorkspaceConfig::Local,
            1 => WorkspaceConfig::Remote { agent_address: address, auth },
            _ => WorkspaceConfig::Container { image, runtime },
        }
    }
}

prop_compose! {
    fn arb_tool_call()(
        id in "[a-zA-Z0-9_-]+",
        name in "[a-z_]+",
        param_key in "[a-z]+",
        param_value in "[a-zA-Z0-9 ]+",
    ) -> ToolCall {
        ToolCall {
            id,
            name,
            parameters: json!({ param_key: param_value }),
        }
    }
}

prop_compose! {
    fn arb_user_content()(
        variant in 0..3usize,
        text in ".*",
        command in "[a-z ]+",
        stdout in ".*",
        stderr in ".*",
        exit_code in 0..255i32,
        target in proptest::option::of("[a-z-]+".prop_map(String::from)),
        response_text in ".*",
    ) -> UserContent {
        match variant {
            0 => UserContent::Text { text },
            1 => UserContent::CommandExecution { command, stdout, stderr, exit_code },
            _ => UserContent::AppCommand {
                command: AppCommandType::Model { target },
                response: Some(CommandResponse::Text(response_text)),
            },
        }
    }
}

prop_compose! {
    fn arb_thought_content()(
        variant in 0..3usize,
        text in ".*",
        signature in "[a-zA-Z0-9]+",
        data in "[a-zA-Z0-9]+",
    ) -> ThoughtContent {
        match variant {
            0 => ThoughtContent::Simple { text },
            1 => ThoughtContent::Signed { text, signature },
            _ => ThoughtContent::Redacted { data },
        }
    }
}

prop_compose! {
    fn arb_assistant_content()(
        variant in 0..3usize,
        text in ".*",
        tool_call in arb_tool_call(),
        thought in arb_thought_content(),
    ) -> AssistantContent {
        match variant {
            0 => AssistantContent::Text { text },
            1 => AssistantContent::ToolCall { tool_call },
            _ => AssistantContent::Thought { thought },
        }
    }
}

prop_compose! {
    fn arb_tool_result()(
        is_success in prop::bool::ANY,
        output in ".*",
        error in ".*",
    ) -> ToolResult {
        if is_success {
            ToolResult::Success { output }
        } else {
            ToolResult::Error { error }
        }
    }
}

prop_compose! {
    fn arb_conversation_message()(
        variant in 0..3usize,
        id in "[a-zA-Z0-9-]+",
        timestamp in 1000000000u64..2000000000u64,
        user_content in prop::collection::vec(arb_user_content(), 0..3),
        assistant_content in prop::collection::vec(arb_assistant_content(), 0..3),
        tool_use_id in "[a-zA-Z0-9_]+",
        result in arb_tool_result(),
    ) -> ConversationMessage {
        match variant {
            0 => ConversationMessage::User { content: user_content, timestamp, id },
            1 => ConversationMessage::Assistant { content: assistant_content, timestamp, id },
            _ => ConversationMessage::Tool { tool_use_id, result, timestamp, id },
        }
    }
}

prop_compose! {
    fn arb_backend_config()(
        variant in 0..4usize,
        name in "[a-z]+",
        endpoint in "[a-z]+://[a-z]+:[0-9]+",
        auth in proptest::option::of(arb_remote_auth()),
        image in "[a-z]+:[a-z]+",
        runtime in prop::sample::select(vec![ContainerRuntime::Docker, ContainerRuntime::Podman]),
        server_name in "[a-z_]+",
        transport in "stdio|http",
        command in "[a-z/]+",
        args in prop::collection::vec("[a-z]+", 0..3),
        tool_filter in arb_tool_filter(),
    ) -> BackendConfig {
        match variant {
            0 => BackendConfig::Local { tool_filter },
            1 => BackendConfig::Remote { name, endpoint, auth, tool_filter },
            2 => BackendConfig::Container { image, runtime, tool_filter },
            _ => BackendConfig::Mcp { server_name, transport, command, args, tool_filter },
        }
    }
}

prop_compose! {
    fn arb_session_tool_config()(
        backends in prop::collection::vec(arb_backend_config(), 0..3),
        metadata_keys in prop::collection::vec("[a-z]+", 0..3),
        metadata_values in prop::collection::vec("[a-zA-Z0-9]+", 0..3),
    ) -> SessionToolConfig {
        let metadata: HashMap<String, String> = metadata_keys.into_iter()
            .zip(metadata_values.into_iter())
            .collect();

        SessionToolConfig {
            backends,
            visibility: ToolVisibility::All,
            approval_policy: ToolApprovalPolicy::AlwaysAsk,
            metadata,
        }
    }
}

prop_compose! {
    fn arb_active_tool()(
        id in "[a-zA-Z0-9-]+",
        name in "[a-z_]+",
    ) -> ActiveTool {
        ActiveTool { id, name }
    }
}

prop_compose! {
    fn arb_cancellation_info()(
        api_call_in_progress in prop::bool::ANY,
        active_tools in prop::collection::vec(arb_active_tool(), 0..3),
        pending_tool_approvals in prop::bool::ANY,
    ) -> CancellationInfo {
        CancellationInfo {
            api_call_in_progress,
            active_tools,
            pending_tool_approvals,
        }
    }
}

// Property tests
proptest! {
    #[test]
    fn prop_tool_filter_roundtrip(filter in arb_tool_filter()) {
        let proto = tool_filter_to_proto(&filter);
        let roundtrip = proto_to_tool_filter(Some(proto));
        prop_assert_eq!(&filter, &roundtrip);
    }

    #[test]
    fn prop_workspace_config_roundtrip(config in arb_workspace_config()) {
        let proto = workspace_config_to_proto(&config);
        let roundtrip = proto_to_workspace_config(proto);

        match (&config, &roundtrip) {
            (WorkspaceConfig::Local, WorkspaceConfig::Local) => {},
            (WorkspaceConfig::Remote { agent_address: a1, auth: auth1 },
             WorkspaceConfig::Remote { agent_address: a2, auth: auth2 }) => {
                prop_assert_eq!(a1, a2);
                match (auth1, auth2) {
                    (Some(RemoteAuth::Bearer { token: t1 }), Some(RemoteAuth::Bearer { token: t2 })) => {
                        prop_assert_eq!(t1, t2);
                    }
                    (Some(RemoteAuth::ApiKey { key: k1 }), Some(RemoteAuth::ApiKey { key: k2 })) => {
                        prop_assert_eq!(k1, k2);
                    }
                    (None, None) => {},
                    _ => prop_assert!(false, "Auth mismatch"),
                }
            }
            (WorkspaceConfig::Container { image: i1, runtime: r1 },
             WorkspaceConfig::Container { image: i2, runtime: r2 }) => {
                prop_assert_eq!(i1, i2);
                // Manually compare ContainerRuntime since it doesn't implement PartialEq
                match (r1, r2) {
                    (ContainerRuntime::Docker, ContainerRuntime::Docker) => {},
                    (ContainerRuntime::Podman, ContainerRuntime::Podman) => {},
                    _ => prop_assert!(false, "Runtime mismatch"),
                }
            }
            _ => prop_assert!(false, "Config variant mismatch"),
        }
    }

    #[test]
    fn prop_tool_call_roundtrip(call in arb_tool_call()) {
        let proto = conductor_proto::agent::ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            parameters_json: call.parameters.to_string(),
        };

        let roundtrip = proto_tool_call_to_core(&proto).unwrap();
        prop_assert_eq!(roundtrip.id, call.id);
        prop_assert_eq!(roundtrip.name, call.name);
        prop_assert_eq!(roundtrip.parameters, call.parameters);
    }

    #[test]
    fn prop_message_roundtrip(message in arb_conversation_message()) {
        let proto = message_to_proto(message.clone());
        let roundtrip = proto_to_message(proto);

        prop_assert!(roundtrip.is_ok());
        let roundtrip = roundtrip.unwrap();

        // Compare the messages field by field since ConversationMessage doesn't implement PartialEq
        prop_assert_eq!(roundtrip.id(), message.id());
        prop_assert_eq!(roundtrip.timestamp(), message.timestamp());

        match (&message, &roundtrip) {
            (ConversationMessage::User { content: c1, .. }, ConversationMessage::User { content: c2, .. }) => {
                prop_assert_eq!(c1.len(), c2.len());
                for (a, b) in c1.iter().zip(c2.iter()) {
                    match (a, b) {
                        (UserContent::Text { text: t1 }, UserContent::Text { text: t2 }) => {
                            prop_assert_eq!(t1, t2);
                        }
                        (UserContent::CommandExecution { command: cmd1, stdout: out1, stderr: err1, exit_code: code1 },
                         UserContent::CommandExecution { command: cmd2, stdout: out2, stderr: err2, exit_code: code2 }) => {
                            prop_assert_eq!(cmd1, cmd2);
                            prop_assert_eq!(out1, out2);
                            prop_assert_eq!(err1, err2);
                            prop_assert_eq!(code1, code2);
                        }
                        (UserContent::AppCommand { command: cmd1, response: resp1 },
                         UserContent::AppCommand { command: cmd2, response: resp2 }) => {
                            if let (AppCommandType::Model { target: t1 }, AppCommandType::Model { target: t2 }) = (cmd1, cmd2) {
                                prop_assert_eq!(t1, t2);
                            }
                            if let (Some(CommandResponse::Text(t1)), Some(CommandResponse::Text(t2))) = (resp1, resp2) {
                                prop_assert_eq!(t1, t2);
                            }
                        }
                        _ => prop_assert!(false, "Content type mismatch"),
                    }
                }
            }
            (ConversationMessage::Assistant { content: c1, .. }, ConversationMessage::Assistant { content: c2, .. }) => {
                prop_assert_eq!(c1.len(), c2.len());
                for (a, b) in c1.iter().zip(c2.iter()) {
                    match (a, b) {
                        (AssistantContent::Text { text: t1 }, AssistantContent::Text { text: t2 }) => {
                            prop_assert_eq!(t1, t2);
                        }
                        (AssistantContent::ToolCall { tool_call: tc1 }, AssistantContent::ToolCall { tool_call: tc2 }) => {
                            prop_assert_eq!(&tc1.id, &tc2.id);
                            prop_assert_eq!(&tc1.name, &tc2.name);
                            prop_assert_eq!(&tc1.parameters, &tc2.parameters);
                        }
                        (AssistantContent::Thought { thought: t1 }, AssistantContent::Thought { thought: t2 }) => {
                            match (t1, t2) {
                                (ThoughtContent::Simple { text: txt1 }, ThoughtContent::Simple { text: txt2 }) => {
                                    prop_assert_eq!(txt1, txt2);
                                }
                                (ThoughtContent::Signed { text: txt1, signature: sig1 },
                                 ThoughtContent::Signed { text: txt2, signature: sig2 }) => {
                                    prop_assert_eq!(txt1, txt2);
                                    prop_assert_eq!(sig1, sig2);
                                }
                                (ThoughtContent::Redacted { data: d1 }, ThoughtContent::Redacted { data: d2 }) => {
                                    prop_assert_eq!(d1, d2);
                                }
                                _ => prop_assert!(false, "Thought type mismatch"),
                            }
                        }
                        _ => prop_assert!(false, "Assistant content type mismatch"),
                    }
                }
            }
            (ConversationMessage::Tool { tool_use_id: id1, result: r1, .. },
             ConversationMessage::Tool { tool_use_id: id2, result: r2, .. }) => {
                prop_assert_eq!(id1, id2);
                match (r1, r2) {
                    (ToolResult::Success { output: o1 }, ToolResult::Success { output: o2 }) => {
                        prop_assert_eq!(o1, o2);
                    }
                    (ToolResult::Error { error: e1 }, ToolResult::Error { error: e2 }) => {
                        prop_assert_eq!(e1, e2);
                    }
                    _ => prop_assert!(false, "Tool result type mismatch"),
                }
            }
            _ => prop_assert!(false, "Message type mismatch"),
        }
    }

    #[test]
    fn prop_session_tool_config_roundtrip(config in arb_session_tool_config()) {
        let proto = session_tool_config_to_proto(&config);
        let roundtrip = proto_to_tool_config(proto);

        // Compare backends
        prop_assert_eq!(config.backends.len(), roundtrip.backends.len());
        for (original, converted) in config.backends.iter().zip(roundtrip.backends.iter()) {
            match (original, converted) {
                (BackendConfig::Local { tool_filter: tf1 }, BackendConfig::Local { tool_filter: tf2 }) => {
                    prop_assert_eq!(tf1, tf2);
                }
                (BackendConfig::Remote { name: n1, endpoint: e1, auth: a1, tool_filter: tf1 },
                 BackendConfig::Remote { name: n2, endpoint: e2, auth: a2, tool_filter: tf2 }) => {
                    prop_assert_eq!(n1, n2);
                    prop_assert_eq!(e1, e2);
                    match (a1, a2) {
                        (Some(RemoteAuth::Bearer { token: t1 }), Some(RemoteAuth::Bearer { token: t2 })) => {
                            prop_assert_eq!(t1, t2);
                        }
                        (Some(RemoteAuth::ApiKey { key: k1 }), Some(RemoteAuth::ApiKey { key: k2 })) => {
                            prop_assert_eq!(k1, k2);
                        }
                        (None, None) => {},
                        _ => prop_assert!(false, "Auth mismatch"),
                    }
                    prop_assert_eq!(tf1, tf2);
                }
                (BackendConfig::Container { image: i1, runtime: r1, tool_filter: tf1 },
                 BackendConfig::Container { image: i2, runtime: r2, tool_filter: tf2 }) => {
                    prop_assert_eq!(i1, i2);
                    match (r1, r2) {
                        (ContainerRuntime::Docker, ContainerRuntime::Docker) => {},
                        (ContainerRuntime::Podman, ContainerRuntime::Podman) => {},
                        _ => prop_assert!(false, "Runtime mismatch"),
                    }
                    prop_assert_eq!(tf1, tf2);
                }
                (BackendConfig::Mcp { server_name: s1, transport: t1, command: c1, args: a1, tool_filter: tf1 },
                 BackendConfig::Mcp { server_name: s2, transport: t2, command: c2, args: a2, tool_filter: tf2 }) => {
                    prop_assert_eq!(s1, s2);
                    prop_assert_eq!(t1, t2);
                    prop_assert_eq!(c1, c2);
                    prop_assert_eq!(a1, a2);
                    prop_assert_eq!(tf1, tf2);
                }
                _ => prop_assert!(false, "Backend variant mismatch"),
            }
        }

        // Compare metadata
        prop_assert_eq!(config.metadata, roundtrip.metadata);
    }

    #[test]
    fn prop_operation_cancelled_roundtrip(info in arb_cancellation_info()) {
        let app_event = AppEvent::OperationCancelled { info: info.clone() };

        // Convert to proto
        let proto_event = app_event_to_server_event(app_event, 42);

        // Convert back
        let roundtrip = server_event_to_app_event(proto_event);

        prop_assert!(roundtrip.is_some());

        if let Some(AppEvent::OperationCancelled { info: roundtrip_info }) = roundtrip {
            prop_assert_eq!(info.api_call_in_progress, roundtrip_info.api_call_in_progress);
            prop_assert_eq!(info.pending_tool_approvals, roundtrip_info.pending_tool_approvals);

            // Most importantly: verify tool IDs are preserved
            prop_assert_eq!(info.active_tools.len(), roundtrip_info.active_tools.len());
            for (original_tool, roundtrip_tool) in info.active_tools.iter().zip(roundtrip_info.active_tools.iter()) {
                prop_assert_eq!(&original_tool.id, &roundtrip_tool.id);
                prop_assert_eq!(&original_tool.name, &roundtrip_tool.name);
            }
        } else {
            prop_assert!(false, "Expected OperationCancelled event");
        }
    }
}
