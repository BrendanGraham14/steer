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
        variant in 0..2usize,
        server_name in "[a-z]+",
        transport in arb_mcp_transport(),
        tool_filter in arb_tool_filter(),
    ) -> BackendConfig {
        match variant {
            0 => BackendConfig::Local { tool_filter },
            _ => BackendConfig::Mcp {
                server_name,
                transport,
                tool_filter,
            },
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
                (BackendConfig::Local { tool_filter: tf1 }, BackendConfig::Local { tool_filter: tf2 }) => {
                    prop_assert_eq!(tf1, tf2);
                }
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
                _ => prop_assert!(false, "Backend variant mismatch"),
            }
        }

        prop_assert_eq!(config.metadata, roundtrip.metadata);
    }
}
