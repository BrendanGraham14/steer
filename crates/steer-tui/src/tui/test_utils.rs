use std::path::PathBuf;

use steer_core::api::Model;
use steer_grpc::AgentClient;

pub async fn local_client_and_server(
    session_dir: Option<PathBuf>,
) -> (AgentClient, tokio::task::JoinHandle<()>) {
    use steer_grpc::local_server::setup_local_grpc;
    let (channel, server_handle) = setup_local_grpc(Model::ClaudeSonnet4_20250514, session_dir)
        .await
        .unwrap();
    let client = AgentClient::from_channel(channel.clone()).await.unwrap();
    (client, server_handle)
}
