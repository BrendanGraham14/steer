use crate::grpc::error::GrpcError;
use crate::grpc::server::AgentServiceImpl;
type Result<T> = std::result::Result<T, GrpcError>;
use conductor_core::session::{SessionManager, SessionManagerConfig};
use conductor_proto::agent::v1::agent_service_server::AgentServiceServer;
use std::sync::Arc;
use tokio::sync::oneshot;
use tonic::transport::{Channel, Server};

/// Creates a localhost gRPC server and client channel
/// This runs both server and client in the same process using localhost TCP
pub async fn create_local_channel(
    session_manager: Arc<SessionManager>,
) -> Result<(Channel, tokio::task::JoinHandle<()>)> {
    // Create a channel for the server's bound address
    let (tx, rx) = oneshot::channel();

    // Create LlmConfigProvider
    #[cfg(not(test))]
    let auth_storage = std::sync::Arc::new(
        conductor_core::auth::DefaultAuthStorage::new()
            .map_err(|e| GrpcError::CoreError(e.into()))?,
    );

    #[cfg(test)]
    let auth_storage = std::sync::Arc::new(conductor_core::test_utils::InMemoryAuthStorage::new());

    let llm_config_provider = conductor_core::config::LlmConfigProvider::new(auth_storage);

    // Create the service
    let service = AgentServiceImpl::new(session_manager, llm_config_provider);
    let svc = AgentServiceServer::new(service);

    // Spawn the server with a listener on localhost
    let server_handle: tokio::task::JoinHandle<()> = tokio::spawn(async move {
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
    let addr = rx
        .await
        .map_err(|e| GrpcError::ChannelError(format!("Failed to receive server address: {e}")))?;

    // Use tonic::transport::Endpoint for proper URI parsing
    let endpoint =
        tonic::transport::Endpoint::try_from(format!("http://{addr}"))?.tcp_nodelay(true);
    let channel = endpoint.connect().await?;

    Ok((channel, server_handle))
}

/// Creates a complete localhost gRPC setup for local mode
/// Returns the channel and a handle to the server task
pub async fn setup_local_grpc(
    default_model: conductor_core::api::Model,
    session_db_path: Option<std::path::PathBuf>,
) -> Result<(Channel, tokio::task::JoinHandle<()>)> {
    // Create session store with the provided configuration
    let store_config =
        conductor_core::utils::session::resolve_session_store_config(session_db_path)?;
    let session_store =
        conductor_core::utils::session::create_session_store_with_config(store_config).await?;

    // Create global event channel (not used in local mode but required)
    let session_manager_config = SessionManagerConfig {
        max_concurrent_sessions: 10,
        default_model,
        auto_persist: true,
    };

    let session_manager = Arc::new(SessionManager::new(session_store, session_manager_config));

    // Create localhost channel
    let (channel, handle) = create_local_channel(session_manager).await?;

    Ok((channel, handle))
}
