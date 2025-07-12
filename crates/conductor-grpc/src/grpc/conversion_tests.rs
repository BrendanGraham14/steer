//! Minimal round-trip tests for conversion helpers that have
//! *both* directions implemented.

use super::conversions::*;
use conductor_core::api::ToolCall;
use conductor_core::app::command::ApprovalType;
use conductor_core::app::conversation::{
    AppCommandType, AssistantContent, CommandResponse, Message as ConversationMessage,
    ThoughtContent, UserContent,
};
use conductor_core::app::{
    AppCommand, AppEvent,
    cancellation::{ActiveTool, CancellationInfo},
};
use conductor_core::session::state::{
    BackendConfig, ContainerRuntime, RemoteAuth, SessionToolConfig, ToolApprovalPolicy, ToolFilter,
    ToolVisibility, WorkspaceConfig,
};
use conductor_tools::{
    ToolError,
    result::{ExternalResult, ToolResult},
};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

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
            0 => WorkspaceConfig::Local { path: std::path::PathBuf::from("/tmp") },
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
        tool_name in "[a-z_]+",
        output in ".*",
        error_variant in 0..4usize,
        error_msg in ".*",
    ) -> ToolResult {
        if is_success {
            ToolResult::External(ExternalResult {
                tool_name,
                payload: output,
            })
        } else {
            match error_variant {
                0 => ToolResult::Error(ToolError::Execution {
                    tool_name: tool_name.clone(),
                    message: error_msg
                }),
                1 => ToolResult::Error(ToolError::UnknownTool(tool_name)),
                2 => ToolResult::Error(ToolError::Cancelled(tool_name)),
                _ => ToolResult::Error(ToolError::InvalidParams(
                    tool_name,
                    error_msg
                )),
            }
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
            0 => ConversationMessage::User { content: user_content, timestamp, id, thread_id: Uuid::new_v4(), parent_message_id: None },
            1 => ConversationMessage::Assistant { content: assistant_content, timestamp, id, thread_id: Uuid::new_v4(), parent_message_id: None },
            _ => ConversationMessage::Tool { tool_use_id, result, timestamp, id, thread_id: Uuid::new_v4(), parent_message_id: None },
        }
    }
}

prop_compose! {
    fn arb_mcp_transport()(
        variant in 0..5usize,
        command in "[a-z/]+",
        args in prop::collection::vec("[a-z]+", 0..3),
        host in "[a-z.]+",
        port in 1024..65535u16,
        path in "/tmp/[a-z]+.sock",
        url in "https://[a-z.]+",
        header_keys in prop::collection::vec("[a-zA-Z-]+", 0..3),
        header_values in prop::collection::vec("[a-zA-Z0-9]+", 0..3),
    ) -> conductor_core::tools::McpTransport {
        let headers: Option<HashMap<String, String>> = if header_keys.is_empty() {
            None
        } else {
            let map: HashMap<String, String> = header_keys.into_iter()
                .zip(header_values.into_iter())
                .collect();
            Some(map)
        };

        match variant {
            0 => conductor_core::tools::McpTransport::Stdio { command, args },
            1 => conductor_core::tools::McpTransport::Tcp { host, port },
            #[cfg(unix)]
            2 => conductor_core::tools::McpTransport::Unix { path },
            #[cfg(not(unix))]
            2 => conductor_core::tools::McpTransport::Stdio { command, args },
            3 => conductor_core::tools::McpTransport::Sse { url: url.clone(), headers: headers.clone() },
            4 => conductor_core::tools::McpTransport::Http { url, headers },
            _ => conductor_core::tools::McpTransport::Http { url, headers },
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
        transport in arb_mcp_transport(),
        tool_filter in arb_tool_filter(),
    ) -> BackendConfig {
        match variant {
            0 => BackendConfig::Local { tool_filter },
            1 => BackendConfig::Remote { name, endpoint, auth, tool_filter },
            2 => BackendConfig::Container { image, runtime, tool_filter },
            _ => BackendConfig::Mcp {
                server_name,
                transport,
                tool_filter
            },
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
            tools: HashMap::new(),
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

prop_compose! {
    fn arb_app_command()(
        variant in 0..4usize,
        user_input in ".*",
        command in prop::sample::select(vec!["model", "clear", "compact"]),
        bash_command in "[a-z ]+",
        tool_id in "[a-zA-Z0-9-]+",
        approved in prop::bool::ANY,
        always in prop::bool::ANY,
    ) -> AppCommand {
        match variant {
            0 => AppCommand::ProcessUserInput(user_input),
            1 => AppCommand::ExecuteCommand(AppCommandType::parse(command).unwrap()),
            2 => AppCommand::ExecuteBashCommand { command: bash_command },
            _ => AppCommand::HandleToolResponse {
                id: tool_id,
                approval: if always {
                    ApprovalType::AlwaysTool
                } else if approved {
                    ApprovalType::Once
                } else {
                    ApprovalType::Denied
                }
            },
        }
    }
}

prop_compose! {
    fn arb_operation()(
        variant in 0..2usize,
        cmd in "[a-z ]+",
    ) -> conductor_core::app::Operation {
        match variant {
            0 => conductor_core::app::Operation::Bash { cmd },
            _ => conductor_core::app::Operation::Compact,
        }
    }
}

prop_compose! {
    fn arb_operation_outcome()(
        variant in 0..2usize,
        elapsed_ms in 1u64..10000,
        exit_code in 0i32..255,
        stderr in ".*",
        compact_msg in ".*",
        result_is_ok in prop::bool::ANY,
    ) -> conductor_core::app::OperationOutcome {
        let elapsed = std::time::Duration::from_millis(elapsed_ms);
        match variant {
            0 => conductor_core::app::OperationOutcome::Bash {
                elapsed,
                result: if result_is_ok {
                    Ok(())
                } else {
                    Err(conductor_core::app::BashError { exit_code, stderr })
                },
            },
            _ => conductor_core::app::OperationOutcome::Compact {
                elapsed,
                result: if result_is_ok {
                    Ok(())
                } else {
                    Err(conductor_core::app::CompactError { message: compact_msg })
                },
            },
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
            (WorkspaceConfig::Local { path: p1 }, WorkspaceConfig::Local { path: p2 }) => {
                prop_assert_eq!(p1, p2);
            },
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
        let proto = conductor_proto::agent::v1::ToolCall {
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
        let proto = message_to_proto(message.clone()).unwrap();
        let roundtrip = proto_to_message(proto).unwrap();

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
                    (ToolResult::External(ext1), ToolResult::External(ext2)) => {
                        prop_assert_eq!(&ext1.tool_name, &ext2.tool_name);
                        prop_assert_eq!(&ext1.payload, &ext2.payload);
                    }
                    (ToolResult::Error(err1), ToolResult::Error(err2)) => {
                        // Compare error types - use discriminant for enum comparison
                        prop_assert_eq!(std::mem::discriminant(err1), std::mem::discriminant(err2));
                        match (err1, err2) {
                            (ToolError::Execution { tool_name: tn1, message: msg1 },
                             ToolError::Execution { tool_name: tn2, message: msg2 }) => {
                                prop_assert_eq!(tn1, tn2);
                                prop_assert_eq!(msg1, msg2);
                            }
                            (ToolError::UnknownTool(tn1), ToolError::UnknownTool(tn2)) => {
                                prop_assert_eq!(tn1, tn2);
                            }
                            (ToolError::Cancelled(tn1), ToolError::Cancelled(tn2)) => {
                                prop_assert_eq!(tn1, tn2);
                            }
                            (ToolError::InvalidParams(tn1, msg1),
                             ToolError::InvalidParams(tn2, msg2)) => {
                                prop_assert_eq!(tn1, tn2);
                                prop_assert_eq!(msg1, msg2);
                            }
                            _ => {} // Other error types we don't generate in our test
                        }
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
                (BackendConfig::Mcp { server_name: s1, transport: t1, tool_filter: tf1 },
                 BackendConfig::Mcp { server_name: s2, transport: t2, tool_filter: tf2 }) => {
                    prop_assert_eq!(s1, s2);
                    // Compare transports manually since McpTransport might not implement PartialEq
                    match (t1, t2) {
                        (conductor_core::tools::McpTransport::Stdio { command: c1, args: a1 },
                         conductor_core::tools::McpTransport::Stdio { command: c2, args: a2 }) => {
                            prop_assert_eq!(c1, c2);
                            prop_assert_eq!(a1, a2);
                        }
                        (conductor_core::tools::McpTransport::Tcp { host: h1, port: p1 },
                         conductor_core::tools::McpTransport::Tcp { host: h2, port: p2 }) => {
                            prop_assert_eq!(h1, h2);
                            prop_assert_eq!(p1, p2);
                        }
                        #[cfg(unix)]
                        (conductor_core::tools::McpTransport::Unix { path: p1 },
                         conductor_core::tools::McpTransport::Unix { path: p2 }) => {
                            prop_assert_eq!(p1, p2);
                        }
                        (conductor_core::tools::McpTransport::Sse { url: u1, headers: h1 },
                         conductor_core::tools::McpTransport::Sse { url: u2, headers: h2 }) => {
                            prop_assert_eq!(u1, u2);
                            prop_assert_eq!(h1, h2);
                        }
                        (conductor_core::tools::McpTransport::Http { url: u1, headers: h1 },
                         conductor_core::tools::McpTransport::Http { url: u2, headers: h2 }) => {
                            prop_assert_eq!(u1, u2);
                            prop_assert_eq!(h1, h2);
                        }
                        _ => prop_assert!(false, "Transport type mismatch"),
                    }
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
        let app_event = AppEvent::OperationCancelled { op_id: None, info: info.clone() };

        // Convert to proto
        let proto_event = app_event_to_server_event(app_event, 42).unwrap();

        // Convert back
        let roundtrip = server_event_to_app_event(proto_event).unwrap();

        if let AppEvent::OperationCancelled { op_id: _, info: roundtrip_info } = roundtrip {
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

    #[test]
    fn prop_app_command_conversion(command in arb_app_command()) {
        let session_id = "test-session-123";

        // Extract expected values before moving command
        enum ExpectedResult {
            ProcessUserInput(String),
            ExecuteCommand(String),
            ExecuteBashCommand(String),
            HandleToolResponse { id: String, approval: ApprovalType },
            NoMessage,
        }

        let expected = match &command {
            AppCommand::ProcessUserInput(text) => ExpectedResult::ProcessUserInput(text.clone()),
            AppCommand::ExecuteCommand(cmd) => ExpectedResult::ExecuteCommand(cmd.as_command_str()),
            AppCommand::ExecuteBashCommand { command } => ExpectedResult::ExecuteBashCommand(command.clone()),
            AppCommand::HandleToolResponse { id, approval } => {
                ExpectedResult::HandleToolResponse {
                    id: id.clone(),
                    approval: approval.clone(),
                }
            }
            _ => ExpectedResult::NoMessage,
        };

        // Now convert the command (moving it)
        let result = convert_app_command_to_client_message(command, session_id);

        // Verify the result
        match expected {
            ExpectedResult::ProcessUserInput(text) => {
                prop_assert!(result.is_ok());
                if let Ok(Some(client_msg)) = result {
                    prop_assert_eq!(client_msg.session_id, session_id);
                    if let Some(conductor_proto::agent::v1::stream_session_request::Message::SendMessage(msg)) = client_msg.message {
                        prop_assert_eq!(msg.message, text);
                        prop_assert_eq!(msg.session_id, session_id);
                    } else {
                        prop_assert!(false, "Expected SendMessage variant");
                    }
                }
            }
            ExpectedResult::ExecuteCommand(cmd) => {
                prop_assert!(result.is_ok());
                if let Ok(Some(client_msg)) = result {
                    prop_assert_eq!(client_msg.session_id, session_id);
                    if let Some(conductor_proto::agent::v1::stream_session_request::Message::ExecuteCommand(msg)) = client_msg.message {
                        prop_assert_eq!(msg.command, cmd);
                        prop_assert_eq!(msg.session_id, session_id);
                    } else {
                        prop_assert!(false, "Expected ExecuteCommand variant");
                    }
                }
            }
            ExpectedResult::ExecuteBashCommand(bash_cmd) => {
                prop_assert!(result.is_ok());
                if let Ok(Some(client_msg)) = result {
                    prop_assert_eq!(client_msg.session_id, session_id);
                    if let Some(conductor_proto::agent::v1::stream_session_request::Message::ExecuteBashCommand(msg)) = client_msg.message {
                        prop_assert_eq!(msg.command, bash_cmd);
                        prop_assert_eq!(msg.session_id, session_id);
                    } else {
                        prop_assert!(false, "Expected ExecuteBashCommand variant");
                    }
                }
            }
            ExpectedResult::HandleToolResponse { id, approval } => {
                prop_assert!(result.is_ok());
                if let Ok(Some(client_msg)) = result {
                    prop_assert_eq!(client_msg.session_id, session_id);
                    if let Some(conductor_proto::agent::v1::stream_session_request::Message::ToolApproval(msg)) = client_msg.message {
                        prop_assert_eq!(msg.tool_call_id, id);
                        match approval {
                            ApprovalType::Denied => {
                                prop_assert_eq!(msg.decision, Some(conductor_proto::agent::v1::ApprovalDecision {
                                    decision_type: Some(conductor_proto::agent::v1::approval_decision::DecisionType::Deny(true))
                                }));
                            }
                            ApprovalType::Once => {
                                prop_assert_eq!(msg.decision, Some(conductor_proto::agent::v1::ApprovalDecision {
                                    decision_type: Some(conductor_proto::agent::v1::approval_decision::DecisionType::Once(true))
                                }));
                            }
                            ApprovalType::AlwaysTool => {
                                prop_assert_eq!(msg.decision, Some(conductor_proto::agent::v1::ApprovalDecision {
                                    decision_type: Some(conductor_proto::agent::v1::approval_decision::DecisionType::AlwaysTool(true))
                                }));
                            }
                            ApprovalType::AlwaysBashPattern(pattern) => {
                                prop_assert_eq!(msg.decision, Some(conductor_proto::agent::v1::ApprovalDecision {
                                    decision_type: Some(conductor_proto::agent::v1::approval_decision::DecisionType::AlwaysBashPattern(pattern.clone()))
                                }));
                            }
                        }
                    } else {
                        prop_assert!(false, "Expected ToolApproval variant");
                    }
                }
            }
            ExpectedResult::NoMessage => {
                prop_assert!(result.is_ok());
                prop_assert!(result.unwrap().is_none());
            }
        }
    }

    #[test]
    fn prop_started_event_roundtrip(op in arb_operation()) {
        let id = uuid::Uuid::new_v4();
        let app_event = AppEvent::Started { id, op: op.clone() };

        // Convert to proto
        let proto_event = app_event_to_server_event(app_event, 42).unwrap();

        // Convert back
        let roundtrip = server_event_to_app_event(proto_event).unwrap();

        if let AppEvent::Started { id: roundtrip_id, op: roundtrip_op } = roundtrip {
            prop_assert_eq!(id, roundtrip_id);

            // Compare the Operation variants
            match (&op, &roundtrip_op) {
                (conductor_core::app::Operation::Bash { cmd: c1 },
                 conductor_core::app::Operation::Bash { cmd: c2 }) => {
                    prop_assert_eq!(c1, c2);
                }
                (conductor_core::app::Operation::Compact,
                 conductor_core::app::Operation::Compact) => {},
                _ => prop_assert!(false, "Operation variant mismatch"),
            }
        } else {
            prop_assert!(false, "Expected Started event");
        }
    }

    #[test]
    fn prop_finished_event_roundtrip(outcome in arb_operation_outcome()) {
        let id = uuid::Uuid::new_v4();
        let app_event = AppEvent::Finished { id, outcome: outcome.clone() };

        // Convert to proto
        let proto_event = app_event_to_server_event(app_event, 42).unwrap();

        // Convert back
        let roundtrip = server_event_to_app_event(proto_event).unwrap();

        if let AppEvent::Finished { id: roundtrip_id, outcome: roundtrip_outcome } = roundtrip {
            prop_assert_eq!(id, roundtrip_id);

            // Compare the OperationOutcome variants
            match (&outcome, &roundtrip_outcome) {
                (conductor_core::app::OperationOutcome::Bash { elapsed: e1, result: r1 },
                 conductor_core::app::OperationOutcome::Bash { elapsed: e2, result: r2 }) => {
                    prop_assert_eq!(e1, e2);
                    match (r1, r2) {
                        (Ok(()), Ok(())) => {},
                        (Err(err1), Err(err2)) => {
                            prop_assert_eq!(err1.exit_code, err2.exit_code);
                            prop_assert_eq!(&err1.stderr, &err2.stderr);
                        }
                        _ => prop_assert!(false, "Bash result mismatch"),
                    }
                }
                (conductor_core::app::OperationOutcome::Compact { elapsed: e1, result: r1 },
                 conductor_core::app::OperationOutcome::Compact { elapsed: e2, result: r2 }) => {
                    prop_assert_eq!(e1, e2);
                    match (r1, r2) {
                        (Ok(()), Ok(())) => {},
                        (Err(err1), Err(err2)) => {
                            prop_assert_eq!(&err1.message, &err2.message);
                        }
                        _ => prop_assert!(false, "Compact result mismatch"),
                    }
                }
                _ => prop_assert!(false, "OperationOutcome variant mismatch"),
            }
        } else {
            prop_assert!(false, "Expected Finished event");
        }
    }
}
