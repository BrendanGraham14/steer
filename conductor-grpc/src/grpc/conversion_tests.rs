//! Minimal round-trip tests for conversion helpers that have
//! *both* directions implemented.

use super::conversions::*;
use conductor_core::session::state::{ToolFilter, WorkspaceConfig, RemoteAuth};
use conductor_core::api::ToolCall;
use serde_json::json;

// Helper to compare ToolFilter values because it doesn't implement PartialEq
fn tool_filter_eq(a: &ToolFilter, b: &ToolFilter) -> bool {
    match (a, b) {
        (ToolFilter::All, ToolFilter::All) => true,
        (ToolFilter::Include(a_vec), ToolFilter::Include(b_vec)) => a_vec == b_vec,
        (ToolFilter::Exclude(a_vec), ToolFilter::Exclude(b_vec)) => a_vec == b_vec,
        _ => false,
    }
}

#[test]
fn tool_filter_roundtrip() {
    let original = ToolFilter::Include(vec!["view".into(), "grep".into()]);
    let proto = tool_filter_to_proto(&original);
    let roundtrip = proto_to_tool_filter(Some(proto));
    assert!(tool_filter_eq(&original, &roundtrip));
}

#[test]
fn workspace_config_roundtrip_local() {
    let original = WorkspaceConfig::Local;
    let proto = workspace_config_to_proto(&original);
    let roundtrip = proto_to_workspace_config(proto);
    matches!(roundtrip, WorkspaceConfig::Local);
}

#[test]
fn workspace_config_roundtrip_remote() {
    let original = WorkspaceConfig::Remote {
        agent_address: "grpc://localhost:50051".into(),
        auth: Some(RemoteAuth::Bearer { token: "abc".into() }),
    };
    let proto = workspace_config_to_proto(&original);
    let roundtrip = proto_to_workspace_config(proto);
    match roundtrip {
        WorkspaceConfig::Remote { agent_address, auth } => {
            assert_eq!(agent_address, "grpc://localhost:50051");
            match auth.unwrap() {
                RemoteAuth::Bearer { token } => assert_eq!(token, "abc"),
                _ => panic!("wrong auth"),
            }
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn tool_call_roundtrip() {
    let call = ToolCall {
        id: "id-1".into(),
        name: "grep".into(),
        parameters: json!({"pattern": "foo"}),
    };
    let proto = conductor_proto::agent::ToolCall {
        id: call.id.clone(),
        name: call.name.clone(),
        parameters_json: call.parameters.to_string(),
    };

    let round = proto_tool_call_to_core(&proto).unwrap();
    assert_eq!(round.id, call.id);
    assert_eq!(round.name, call.name);
    assert_eq!(round.parameters, call.parameters);
}