use steer_grpc::local_server::setup_local_grpc;
use steer_proto::agent::v1::agent_service_client::AgentServiceClient;
use steer_proto::agent::v1::auth_progress::State;
use steer_proto::agent::v1::{GetAuthProgressRequest, StartAuthRequest};
use tempfile::TempDir;

#[tokio::test]
async fn test_auth_progress_polling_without_input() {
    let workspace_root = TempDir::new().expect("temp workspace");
    let default_model = steer_core::config::model::builtin::claude_sonnet_4_5();

    let (channel, server_handle) = setup_local_grpc(
        default_model,
        None,
        Some(workspace_root.path().to_path_buf()),
    )
    .await
    .expect("setup local grpc");

    let mut client = AgentServiceClient::new(channel);

    let start = client
        .start_auth(StartAuthRequest {
            provider_id: "anthropic".to_string(),
        })
        .await
        .expect("start auth")
        .into_inner();

    assert!(!start.flow_id.is_empty());
    let start_state = start.progress.as_ref().and_then(|p| p.state.as_ref());
    assert!(matches!(
        start_state,
        Some(State::OauthStarted(_)) | Some(State::NeedInput(_)) | Some(State::InProgress(_))
    ));

    let progress = client
        .get_auth_progress(GetAuthProgressRequest {
            flow_id: start.flow_id.clone(),
        })
        .await
        .expect("get auth progress")
        .into_inner()
        .progress
        .expect("progress response");

    assert!(!matches!(progress.state, Some(State::Error(_))));

    server_handle.abort();
}

#[tokio::test]
async fn test_openai_auth_progress_polling_without_input() {
    let workspace_root = TempDir::new().expect("temp workspace");
    let default_model = steer_core::config::model::builtin::claude_sonnet_4_5();

    let (channel, server_handle) = setup_local_grpc(
        default_model,
        None,
        Some(workspace_root.path().to_path_buf()),
    )
    .await
    .expect("setup local grpc");

    let mut client = AgentServiceClient::new(channel);

    let start = client
        .start_auth(StartAuthRequest {
            provider_id: "openai".to_string(),
        })
        .await
        .expect("start auth")
        .into_inner();

    assert!(!start.flow_id.is_empty());
    let start_state = start.progress.as_ref().and_then(|p| p.state.as_ref());
    assert!(matches!(
        start_state,
        Some(State::OauthStarted(_)) | Some(State::NeedInput(_)) | Some(State::InProgress(_))
    ));

    let progress = client
        .get_auth_progress(GetAuthProgressRequest {
            flow_id: start.flow_id.clone(),
        })
        .await
        .expect("get auth progress")
        .into_inner()
        .progress
        .expect("progress response");

    assert!(!matches!(progress.state, Some(State::Error(_))));

    server_handle.abort();
}
