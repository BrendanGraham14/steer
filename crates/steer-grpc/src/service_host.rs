use crate::grpc::error::GrpcError;
type Result<T> = std::result::Result<T, GrpcError>;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tonic::transport::Server;
use tracing::{error, info};

use crate::grpc::server::AgentServiceImpl;
use steer_core::auth::storage::AuthStorage;
use steer_core::session::{SessionManager, SessionManagerConfig, SessionStore};
use steer_proto::agent::v1::agent_service_server::AgentServiceServer;

/// Configuration for the ServiceHost
#[derive(Clone)]
pub struct ServiceHostConfig {
    /// Path to the session database
    pub db_path: std::path::PathBuf,
    /// Session manager configuration
    pub session_manager_config: SessionManagerConfig,
    /// gRPC server bind address
    pub bind_addr: SocketAddr,
    /// Auth storage
    pub auth_storage: Arc<dyn AuthStorage>,
}

impl std::fmt::Debug for ServiceHostConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceHostConfig")
            .field("db_path", &self.db_path)
            .field("session_manager_config", &self.session_manager_config)
            .field("bind_addr", &self.bind_addr)
            .field("auth_storage", &"Arc<dyn AuthStorage>")
            .finish()
    }
}

impl ServiceHostConfig {
    /// Create a new ServiceHostConfig with default auth storage
    pub fn new(
        db_path: std::path::PathBuf,
        session_manager_config: SessionManagerConfig,
        bind_addr: SocketAddr,
    ) -> Result<Self> {
        let auth_storage = Arc::new(
            steer_core::auth::DefaultAuthStorage::new()
                .map_err(|e| GrpcError::CoreError(e.into()))?,
        );

        Ok(Self {
            db_path,
            session_manager_config,
            bind_addr,
            auth_storage,
        })
    }
}

/// Main orchestrator for the service host system
/// Manages the gRPC server, SessionManager, and component lifecycle
pub struct ServiceHost {
    session_manager: Arc<SessionManager>,
    server_handle: Option<JoinHandle<Result<()>>>,
    cleanup_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    config: ServiceHostConfig,
}

impl ServiceHost {
    /// Create a new ServiceHost with the given configuration
    pub async fn new(config: ServiceHostConfig) -> Result<Self> {
        // Initialize session store
        let store = create_session_store(&config.db_path).await?;

        // Create session manager
        let session_manager = Arc::new(SessionManager::new(
            store,
            config.session_manager_config.clone(),
        ));

        info!(
            "ServiceHost initialized with database at {:?}",
            config.db_path
        );

        Ok(Self {
            session_manager,
            server_handle: None,
            cleanup_handle: None,
            shutdown_tx: None,
            config,
        })
    }

    /// Start the gRPC server
    pub async fn start(&mut self) -> Result<()> {
        if self.server_handle.is_some() {
            return Err(GrpcError::InvalidSessionState {
                reason: "Server is already running".to_string(),
            });
        }

        // Use auth storage from config
        let llm_config_provider =
            steer_core::config::LlmConfigProvider::new(self.config.auth_storage.clone());

        let service = AgentServiceImpl::new(self.session_manager.clone(), llm_config_provider);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let addr = self.config.bind_addr;

        info!("Starting gRPC server on {}", addr);

        let server_handle = tokio::spawn(async move {
            Server::builder()
                .add_service(AgentServiceServer::new(service))
                .serve_with_shutdown(addr, async {
                    shutdown_rx.await.ok();
                    info!("gRPC server shutdown signal received");
                })
                .await
                .map_err(GrpcError::ConnectionFailed)
        });

        // Start periodic cleanup task
        let session_manager = self.session_manager.clone();
        let cleanup_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300)); // 5 minutes
            loop {
                interval.tick().await;

                // Clean up sessions that have been idle for more than 30 minutes
                let idle_duration = chrono::Duration::minutes(30);
                match session_manager
                    .cleanup_inactive_sessions(idle_duration)
                    .await
                {
                    0 => {} // No sessions cleaned, don't log
                    count => info!("Cleaned up {} inactive sessions", count),
                }
            }
        });

        self.server_handle = Some(server_handle);
        self.cleanup_handle = Some(cleanup_handle);
        self.shutdown_tx = Some(shutdown_tx);

        info!("gRPC server listening on {}", addr);
        Ok(())
    }

    /// Shutdown the server gracefully
    pub async fn shutdown(mut self) -> Result<()> {
        info!("Initiating ServiceHost shutdown");

        // Send shutdown signal to server
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Abort cleanup task
        if let Some(cleanup_handle) = self.cleanup_handle.take() {
            cleanup_handle.abort();
        }

        // Wait for server to finish
        if let Some(server_handle) = self.server_handle.take() {
            match server_handle.await {
                Ok(Ok(())) => info!("gRPC server shut down successfully"),
                Ok(Err(e)) => error!("gRPC server error during shutdown: {}", e),
                Err(e) => error!("Failed to join server task: {}", e),
            }
        }

        // Clean up active sessions
        let active_sessions = self.session_manager.get_active_sessions().await;
        for session_id in active_sessions {
            if let Err(e) = self.session_manager.suspend_session(&session_id).await {
                error!(
                    "Failed to suspend session {} during shutdown: {}",
                    session_id, e
                );
            }
        }

        info!("ServiceHost shutdown complete");
        Ok(())
    }

    /// Get a reference to the session manager
    pub fn session_manager(&self) -> &Arc<SessionManager> {
        &self.session_manager
    }

    /// Wait for the server to finish (blocks until shutdown)
    pub async fn wait(&mut self) -> Result<()> {
        if let Some(server_handle) = &mut self.server_handle {
            match server_handle.await {
                Ok(result) => result,
                Err(e) => Err(GrpcError::StreamError(format!("Server task panicked: {e}"))),
            }
        } else {
            Err(GrpcError::InvalidSessionState {
                reason: "Server is not running".to_string(),
            })
        }
    }
}

/// Create a session store from the given database path
async fn create_session_store(db_path: &std::path::Path) -> Result<Arc<dyn SessionStore>> {
    use steer_core::session::SessionStoreConfig;
    use steer_core::utils::session::create_session_store_with_config;

    let config = SessionStoreConfig::sqlite(db_path.to_path_buf());
    create_session_store_with_config(config)
        .await
        .map_err(|e| GrpcError::InvalidSessionState {
            reason: format!("Failed to create session store: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use steer_core::config::provider::ProviderId;
    use tempfile::TempDir;

    fn create_test_config() -> (ServiceHostConfig, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let config = ServiceHostConfig {
            db_path,
            session_manager_config: SessionManagerConfig {
                max_concurrent_sessions: 10,
                default_model: (
                    ProviderId::Anthropic,
                    "claude-3-7-sonnet-20250219".to_string(),
                ),
                auto_persist: true,
            },
            bind_addr: "127.0.0.1:0".parse().unwrap(), // Use port 0 for testing
            auth_storage: Arc::new(steer_core::test_utils::InMemoryAuthStorage::new()),
        };

        (config, temp_dir)
    }

    #[tokio::test]
    async fn test_service_host_creation() {
        let (config, _temp_dir) = create_test_config();

        let host = ServiceHost::new(config).await.unwrap();

        // Verify session manager was created
        assert_eq!(host.session_manager().get_active_sessions().await.len(), 0);
    }

    #[tokio::test]
    async fn test_service_host_lifecycle() {
        let (mut config, _temp_dir) = create_test_config();
        config.bind_addr = "127.0.0.1:0".parse().unwrap(); // Use any available port

        let mut host = ServiceHost::new(config).await.unwrap();

        // Start server
        host.start().await.unwrap();

        // Verify it's running
        assert!(host.server_handle.is_some());

        // Shutdown
        host.shutdown().await.unwrap();
    }
}
