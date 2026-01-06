use crate::grpc::RuntimeAgentService;
use crate::grpc::error::GrpcError;
type Result<T> = std::result::Result<T, GrpcError>;
use std::sync::Arc;
use steer_core::api::Client as ApiClient;
use steer_core::app::domain::runtime::{RuntimeConfig, RuntimeService};
use steer_core::app::domain::session::{InMemoryEventStore, SessionCatalog};
use steer_core::catalog::CatalogConfig;
use steer_core::config::model::ModelId;
use steer_core::tools::ToolSystemBuilder;
use steer_proto::agent::v1::agent_service_server::AgentServiceServer;
use tokio::sync::oneshot;
use tonic::transport::{Channel, Server};

pub async fn create_local_channel(
    runtime_service: &RuntimeService,
    catalog: Arc<dyn SessionCatalog>,
    model_registry: Arc<steer_core::model_registry::ModelRegistry>,
    provider_registry: Arc<steer_core::auth::ProviderRegistry>,
    llm_config_provider: steer_core::config::LlmConfigProvider,
) -> Result<(Channel, tokio::task::JoinHandle<()>)> {
    let (tx, rx) = oneshot::channel();

    let service = RuntimeAgentService::new(
        runtime_service.handle(),
        catalog,
        llm_config_provider,
        model_registry,
        provider_registry,
    );
    let svc = AgentServiceServer::new(service);

    let server_handle: tokio::task::JoinHandle<()> = tokio::spawn(async move {
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        tx.send(local_addr).unwrap();

        Server::builder()
            .add_service(svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .expect("Failed to run localhost server");
    });

    let addr = rx
        .await
        .map_err(|e| GrpcError::ChannelError(format!("Failed to receive server address: {e}")))?;

    let endpoint =
        tonic::transport::Endpoint::try_from(format!("http://{addr}"))?.tcp_nodelay(true);
    let channel = endpoint.connect().await?;

    Ok((channel, server_handle))
}

pub struct LocalGrpcSetup {
    pub channel: Channel,
    pub server_handle: tokio::task::JoinHandle<()>,
    pub runtime_service: RuntimeService,
}

pub async fn setup_local_grpc_with_catalog(
    default_model: ModelId,
    session_db_path: Option<std::path::PathBuf>,
    catalog_config: CatalogConfig,
) -> Result<LocalGrpcSetup> {
    let (event_store, catalog): (
        Arc<dyn steer_core::app::domain::session::EventStore>,
        Arc<dyn SessionCatalog>,
    ) = if let Some(db_path) = session_db_path {
        let sqlite_store = Arc::new(
            steer_core::app::domain::session::SqliteEventStore::new(&db_path)
                .await
                .map_err(|e| GrpcError::InvalidSessionState {
                    reason: format!("Failed to create event store: {e}"),
                })?,
        );
        (sqlite_store.clone(), sqlite_store)
    } else {
        (
            Arc::new(InMemoryEventStore::new()),
            Arc::new(InMemoryCatalog::new()),
        )
    };

    let model_registry = Arc::new(
        steer_core::model_registry::ModelRegistry::load(&catalog_config.catalog_paths)
            .map_err(GrpcError::CoreError)?,
    );

    let provider_registry = Arc::new(
        steer_core::auth::ProviderRegistry::load(&catalog_config.catalog_paths)
            .map_err(GrpcError::CoreError)?,
    );

    #[cfg(not(test))]
    let auth_storage = std::sync::Arc::new(
        steer_core::auth::DefaultAuthStorage::new().map_err(|e| GrpcError::CoreError(e.into()))?,
    );

    #[cfg(test)]
    let auth_storage = std::sync::Arc::new(steer_core::test_utils::InMemoryAuthStorage::new());

    let llm_config_provider = steer_core::config::LlmConfigProvider::new(auth_storage);

    let api_client = Arc::new(ApiClient::new_with_deps(
        llm_config_provider.clone(),
        provider_registry.clone(),
        model_registry.clone(),
    ));

    let workspace =
        steer_core::workspace::create_workspace(&steer_core::workspace::WorkspaceConfig::Local {
            path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        })
        .await
        .map_err(|e| GrpcError::InvalidSessionState {
            reason: format!("Failed to create workspace: {e}"),
        })?;

    let tool_executor = ToolSystemBuilder::new(
        workspace,
        event_store.clone(),
        api_client.clone(),
        model_registry.clone(),
    )
    .build();

    let runtime_config = RuntimeConfig::new(default_model);

    let runtime_service =
        RuntimeService::spawn(event_store, api_client, tool_executor, runtime_config);

    let (channel, server_handle) = create_local_channel(
        &runtime_service,
        catalog,
        model_registry,
        provider_registry,
        llm_config_provider,
    )
    .await?;

    Ok(LocalGrpcSetup {
        channel,
        server_handle,
        runtime_service,
    })
}

pub async fn setup_local_grpc(
    default_model: ModelId,
    session_db_path: Option<std::path::PathBuf>,
) -> Result<(Channel, tokio::task::JoinHandle<()>)> {
    let setup =
        setup_local_grpc_with_catalog(default_model, session_db_path, CatalogConfig::default())
            .await?;
    Ok((setup.channel, setup.server_handle))
}

struct InMemoryCatalog {
    sessions: tokio::sync::RwLock<
        std::collections::HashMap<
            steer_core::app::domain::types::SessionId,
            steer_core::session::state::SessionConfig,
        >,
    >,
}

impl InMemoryCatalog {
    fn new() -> Self {
        Self {
            sessions: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl SessionCatalog for InMemoryCatalog {
    async fn get_session_config(
        &self,
        session_id: steer_core::app::domain::types::SessionId,
    ) -> std::result::Result<
        Option<steer_core::session::state::SessionConfig>,
        steer_core::app::domain::session::SessionCatalogError,
    > {
        let sessions = self.sessions.read().await;
        Ok(sessions.get(&session_id).cloned())
    }

    async fn get_session_summary(
        &self,
        _session_id: steer_core::app::domain::types::SessionId,
    ) -> std::result::Result<
        Option<steer_core::app::domain::session::SessionSummary>,
        steer_core::app::domain::session::SessionCatalogError,
    > {
        Ok(None)
    }

    async fn list_sessions(
        &self,
        _filter: steer_core::app::domain::session::SessionFilter,
    ) -> std::result::Result<
        Vec<steer_core::app::domain::session::SessionSummary>,
        steer_core::app::domain::session::SessionCatalogError,
    > {
        Ok(vec![])
    }

    async fn update_session_catalog(
        &self,
        session_id: steer_core::app::domain::types::SessionId,
        config: Option<&steer_core::session::state::SessionConfig>,
        _increment_message_count: bool,
        _new_model: Option<&str>,
    ) -> std::result::Result<(), steer_core::app::domain::session::SessionCatalogError> {
        if let Some(cfg) = config {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id, cfg.clone());
        }
        Ok(())
    }
}
