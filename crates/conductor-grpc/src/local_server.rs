use crate::grpc::server::AgentServiceImpl;
use anyhow::Result;
use conductor_core::events::StreamEventWithMetadata;
use conductor_core::session::{SessionManager, SessionManagerConfig};
use conductor_proto::agent::agent_service_server::AgentServiceServer;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tonic::transport::{Channel, Server};

/// Creates a localhost gRPC server and client channel
/// This runs both server and client in the same process using localhost TCP
pub async fn create_local_channel(session_manager: Arc<SessionManager>) -> Result<Channel> {
    // Create a channel for the server's bound address
    let (tx, rx) = oneshot::channel();

    // Create the service
    let service = AgentServiceImpl::new(session_manager);
    let svc = AgentServiceServer::new(service);

    // Spawn the server with a listener on localhost
    let _server_handle = tokio::spawn(async move {
        // Bind to port 0 to get a random available port
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        // Send the bound address back
        tx.send(local_addr).unwrap();

        // Run the server
        Server::builder()
            .add_service(svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .expect("Failed to run localhost server");
    });

    // Wait for the server to be ready and get its address
    let addr = rx.await?;

    // Use tonic::transport::Endpoint for proper URI parsing
    let endpoint =
        tonic::transport::Endpoint::try_from(format!("http://{}", addr))?.tcp_nodelay(true);
    let channel = endpoint.connect().await?;

    Ok(channel)
}

/// Creates a complete localhost gRPC setup for local mode
/// Returns the channel for use by the TUI
pub async fn setup_local_grpc(
    _llm_config: conductor_core::config::LlmConfig,
    default_model: conductor_core::api::Model,
) -> Result<Channel> {
    // Create session store
    let session_store = conductor_core::utils::session::create_session_store().await?;

    // Create global event channel (not used in local mode but required)
    let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

    let session_manager_config = SessionManagerConfig {
        max_concurrent_sessions: 10,
        default_model,
        auto_persist: true,
    };

    let session_manager = Arc::new(SessionManager::new(
        session_store,
        session_manager_config,
        global_event_tx,
    ));

    // Create localhost channel
    let channel = create_local_channel(session_manager).await?;

    Ok(channel)
}
