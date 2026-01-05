use crate::grpc::error::GrpcError;
type Result<T> = std::result::Result<T, GrpcError>;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tonic::transport::Server;
use tracing::{error, info};

use crate::grpc::RuntimeAgentService;
use steer_core::api::Client as ApiClient;
use steer_core::app::domain::runtime::{RuntimeConfig, RuntimeHandle, RuntimeService};
use steer_core::app::domain::session::{SessionCatalog, SqliteEventStore};
use steer_core::auth::storage::AuthStorage;
use steer_core::catalog::CatalogConfig;
use steer_core::config::model::ModelId;
use steer_core::tools::ToolExecutor;
use steer_proto::agent::v1::agent_service_server::AgentServiceServer;

#[derive(Clone)]
pub struct ServiceHostConfig {
    pub db_path: std::path::PathBuf,
    pub default_model: ModelId,
    pub bind_addr: SocketAddr,
    pub auth_storage: Arc<dyn AuthStorage>,
    pub catalog_config: CatalogConfig,
}

impl std::fmt::Debug for ServiceHostConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceHostConfig")
            .field("db_path", &self.db_path)
            .field("default_model", &self.default_model)
            .field("bind_addr", &self.bind_addr)
            .field("auth_storage", &"Arc<dyn AuthStorage>")
            .field("catalog_config", &self.catalog_config)
            .finish()
    }
}

impl ServiceHostConfig {
    pub fn new(
        db_path: std::path::PathBuf,
        default_model: ModelId,
        bind_addr: SocketAddr,
    ) -> Result<Self> {
        let auth_storage = Arc::new(
            steer_core::auth::DefaultAuthStorage::new()
                .map_err(|e| GrpcError::CoreError(e.into()))?,
        );

        Ok(Self {
            db_path,
            default_model,
            bind_addr,
            auth_storage,
            catalog_config: CatalogConfig::default(),
        })
    }

    pub fn with_catalog(
        db_path: std::path::PathBuf,
        default_model: ModelId,
        bind_addr: SocketAddr,
        catalog_config: CatalogConfig,
    ) -> Result<Self> {
        let auth_storage = Arc::new(
            steer_core::auth::DefaultAuthStorage::new()
                .map_err(|e| GrpcError::CoreError(e.into()))?,
        );

        Ok(Self {
            db_path,
            default_model,
            bind_addr,
            auth_storage,
            catalog_config,
        })
    }
}

pub struct ServiceHost {
    runtime_service: RuntimeService,
    runtime_handle: RuntimeHandle,
    catalog: Arc<dyn SessionCatalog>,
    model_registry: Arc<steer_core::model_registry::ModelRegistry>,
    provider_registry: Arc<steer_core::auth::ProviderRegistry>,
    llm_config_provider: steer_core::config::LlmConfigProvider,
    server_handle: Option<JoinHandle<Result<()>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    config: ServiceHostConfig,
}

impl ServiceHost {
    pub async fn new(config: ServiceHostConfig) -> Result<Self> {
        let event_store = Arc::new(SqliteEventStore::new(&config.db_path).await.map_err(|e| {
            GrpcError::InvalidSessionState {
                reason: format!("Failed to create event store: {e}"),
            }
        })?);

        let catalog: Arc<dyn SessionCatalog> = event_store.clone();

        let model_registry = Arc::new(
            steer_core::model_registry::ModelRegistry::load(&config.catalog_config.catalog_paths)
                .map_err(|e| GrpcError::InvalidSessionState {
                reason: format!("Failed to load model registry: {e}"),
            })?,
        );

        let provider_registry = Arc::new(
            steer_core::auth::ProviderRegistry::load(&config.catalog_config.catalog_paths)
                .map_err(|e| GrpcError::InvalidSessionState {
                    reason: format!("Failed to load provider registry: {e}"),
                })?,
        );

        let llm_config_provider =
            steer_core::config::LlmConfigProvider::new(config.auth_storage.clone());

        let api_client = Arc::new(ApiClient::new_with_deps(
            llm_config_provider.clone(),
            provider_registry.clone(),
            model_registry.clone(),
        ));

        let workspace = steer_core::workspace::create_workspace(
            &steer_core::workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            },
        )
        .await
        .map_err(|e| GrpcError::InvalidSessionState {
            reason: format!("Failed to create workspace: {e}"),
        })?;

        let tool_executor = Arc::new(ToolExecutor::with_components(
            workspace,
            Arc::new(steer_core::tools::BackendRegistry::new()),
            Arc::new(steer_core::app::validation::ValidatorRegistry::new()),
        ));

        let runtime_config = RuntimeConfig::new(config.default_model.clone());

        let runtime_service =
            RuntimeService::spawn(event_store, api_client, tool_executor, runtime_config);

        let runtime_handle = runtime_service.handle();

        info!(
            "ServiceHost initialized with database at {:?}",
            config.db_path
        );

        Ok(Self {
            runtime_service,
            runtime_handle,
            catalog,
            model_registry,
            provider_registry,
            llm_config_provider,
            server_handle: None,
            shutdown_tx: None,
            config,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        if self.server_handle.is_some() {
            return Err(GrpcError::InvalidSessionState {
                reason: "Server is already running".to_string(),
            });
        }

        let service = RuntimeAgentService::new(
            self.runtime_handle.clone(),
            self.catalog.clone(),
            self.llm_config_provider.clone(),
            self.model_registry.clone(),
            self.provider_registry.clone(),
        );

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

        self.server_handle = Some(server_handle);
        self.shutdown_tx = Some(shutdown_tx);

        info!("gRPC server listening on {}", addr);
        Ok(())
    }

    pub async fn shutdown(mut self) -> Result<()> {
        info!("Initiating ServiceHost shutdown");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        if let Some(server_handle) = self.server_handle.take() {
            match server_handle.await {
                Ok(Ok(())) => info!("gRPC server shut down successfully"),
                Ok(Err(e)) => error!("gRPC server error during shutdown: {}", e),
                Err(e) => error!("Failed to join server task: {}", e),
            }
        }

        self.runtime_service.shutdown().await;

        info!("ServiceHost shutdown complete");
        Ok(())
    }

    pub fn runtime_handle(&self) -> &RuntimeHandle {
        &self.runtime_handle
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config() -> (ServiceHostConfig, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let config = ServiceHostConfig {
            db_path,
            default_model: steer_core::config::model::builtin::claude_sonnet_4_20250514(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            auth_storage: Arc::new(steer_core::test_utils::InMemoryAuthStorage::new()),
            catalog_config: CatalogConfig::default(),
        };

        (config, temp_dir)
    }

    #[tokio::test]
    async fn test_service_host_creation() {
        let (config, _temp_dir) = create_test_config();

        let host = ServiceHost::new(config).await.unwrap();

        let sessions = host.runtime_handle.list_all_sessions().await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_service_host_lifecycle() {
        let (mut config, _temp_dir) = create_test_config();
        config.bind_addr = "127.0.0.1:0".parse().unwrap();

        let mut host = ServiceHost::new(config).await.unwrap();

        host.start().await.unwrap();
        assert!(host.server_handle.is_some());

        host.shutdown().await.unwrap();
    }
}
