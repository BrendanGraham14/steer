use crate::grpc::RuntimeAgentService;
use crate::grpc::error::GrpcError;
type Result<T> = std::result::Result<T, GrpcError>;
use std::sync::Arc;
use steer_core::api::Client as ApiClient;
use steer_core::app::domain::runtime::RuntimeService;
use steer_core::app::domain::session::{InMemoryEventStore, SessionCatalog};
use steer_core::catalog::CatalogConfig;
use steer_core::config::model::ModelId;
use steer_core::tools::ToolSystemBuilder;
use steer_proto::agent::v1::agent_service_server::AgentServiceServer;
use steer_workspace::{LocalEnvironmentManager, LocalWorkspaceManager};
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

    let workspace_root =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let workspace_manager = Arc::new(
        LocalWorkspaceManager::new(workspace_root.clone())
            .await
            .map_err(|e| GrpcError::InvalidSessionState {
                reason: format!("Failed to create workspace manager: {e}"),
            })?,
    );
    let environment_manager = Arc::new(LocalEnvironmentManager::new(workspace_root));

    let service = RuntimeAgentService::new(
        runtime_service.handle(),
        catalog,
        llm_config_provider,
        model_registry,
        provider_registry,
        environment_manager,
        workspace_manager,
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
    _default_model: ModelId,
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
        let in_memory_store = Arc::new(InMemoryEventStore::new());
        (in_memory_store.clone(), in_memory_store)
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

    let workspace_root =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let workspace =
        steer_core::workspace::create_workspace(&steer_core::workspace::WorkspaceConfig::Local {
            path: workspace_root.clone(),
        })
        .await
        .map_err(|e| GrpcError::InvalidSessionState {
            reason: format!("Failed to create workspace: {e}"),
        })?;
    let workspace_manager = Arc::new(
        LocalWorkspaceManager::new(workspace_root)
            .await
            .map_err(|e| GrpcError::InvalidSessionState {
                reason: format!("Failed to create workspace manager: {e}"),
            })?,
    );

    let tool_executor = ToolSystemBuilder::new(
        workspace,
        event_store.clone(),
        api_client.clone(),
        model_registry.clone(),
    )
    .with_workspace_manager(workspace_manager)
    .build();

    let runtime_service = RuntimeService::spawn(event_store, api_client, tool_executor);

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
#[cfg(test)]
mod tests {
    use super::*;
    use steer_core::api::error::ApiError;
    use steer_core::api::provider::{CompletionResponse, Provider};
    use steer_core::app::conversation::AssistantContent;
    use steer_core::app::domain::action::Action;
    use steer_core::app::domain::types::OpId;
    use steer_core::config::model::ModelId;
    use steer_core::session::state::SessionConfig;
    use steer_proto::agent::v1::{
        CompactSessionRequest, ExecuteBashCommandRequest, SendMessageRequest,
        SubscribeSessionEventsRequest, agent_service_client::AgentServiceClient,
    };
    use tokio::time::{Duration, timeout};
    use tokio_util::sync::CancellationToken;
    use tonic::Code;

    const STUB_RESPONSE: &str = "stub response";

    #[derive(Clone)]
    struct StubProvider;

    #[async_trait::async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &'static str {
            "stub"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<steer_core::app::conversation::Message>,
            _system: Option<String>,
            _tools: Option<Vec<steer_tools::ToolSchema>>,
            _call_options: Option<steer_core::config::model::ModelParameters>,
            _token: CancellationToken,
        ) -> std::result::Result<CompletionResponse, ApiError> {
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: STUB_RESPONSE.to_string(),
                }],
            })
        }
    }

    async fn setup_local_grpc_with_stub_provider(default_model: ModelId) -> Result<LocalGrpcSetup> {
        let in_memory_store = Arc::new(InMemoryEventStore::new());
        let event_store: Arc<dyn steer_core::app::domain::session::EventStore> =
            in_memory_store.clone();
        let catalog: Arc<dyn SessionCatalog> = in_memory_store;
        let catalog_config = CatalogConfig::default();

        let model_registry = Arc::new(
            steer_core::model_registry::ModelRegistry::load(&catalog_config.catalog_paths)
                .map_err(GrpcError::CoreError)?,
        );
        let provider_registry = Arc::new(
            steer_core::auth::ProviderRegistry::load(&catalog_config.catalog_paths)
                .map_err(GrpcError::CoreError)?,
        );

        let auth_storage = Arc::new(steer_core::test_utils::InMemoryAuthStorage::new());
        let llm_config_provider = steer_core::config::LlmConfigProvider::new(auth_storage);

        let api_client = Arc::new(ApiClient::new_with_deps(
            llm_config_provider.clone(),
            provider_registry.clone(),
            model_registry.clone(),
        ));
        api_client.insert_test_provider(default_model.0.clone(), Arc::new(StubProvider));

        let workspace = steer_core::workspace::create_workspace(
            &steer_core::workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            },
        )
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

        let runtime_service = RuntimeService::spawn(event_store, api_client, tool_executor);

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

    async fn next_event(
        stream: &mut tonic::Streaming<steer_proto::agent::v1::SessionEvent>,
    ) -> steer_proto::agent::v1::SessionEvent {
        timeout(Duration::from_secs(5), stream.message())
            .await
            .expect("timeout")
            .expect("stream ok")
            .expect("event")
    }

    async fn wait_for_processing_completed(
        stream: &mut tonic::Streaming<steer_proto::agent::v1::SessionEvent>,
        op_id: &str,
    ) {
        loop {
            let event = next_event(stream).await;
            if matches!(
                event.event,
                Some(steer_proto::agent::v1::session_event::Event::ProcessingCompleted(
                    ref e
                )) if e.op_id == op_id
            ) {
                break;
            }
        }
    }

    #[tokio::test]
    async fn test_since_sequence_replay_returns_persisted_events() {
        let setup = setup_local_grpc_with_catalog(
            steer_core::config::model::builtin::claude_sonnet_4_5(),
            None,
            CatalogConfig::default(),
        )
        .await
        .expect("local grpc setup");

        let session_id = setup
            .runtime_service
            .handle()
            .create_session(SessionConfig::read_only(
                steer_core::config::model::builtin::claude_sonnet_4_5(),
            ))
            .await
            .expect("create session");

        let op_id = OpId::new();
        setup
            .runtime_service
            .handle()
            .dispatch_action(
                session_id,
                Action::ModelResponseError {
                    session_id,
                    op_id,
                    error: "boom".to_string(),
                },
            )
            .await
            .expect("dispatch action");

        let mut client = AgentServiceClient::new(setup.channel.clone());
        let request = tonic::Request::new(SubscribeSessionEventsRequest {
            session_id: session_id.to_string(),
            since_sequence: Some(0),
        });

        let mut stream = client
            .subscribe_session_events(request)
            .await
            .expect("subscribe")
            .into_inner();

        let mut events = Vec::new();
        for _ in 0..2 {
            let event = timeout(Duration::from_secs(2), stream.message())
                .await
                .expect("timeout")
                .expect("stream ok")
                .expect("event");
            events.push(event);
        }

        assert!(events.iter().any(|evt| matches!(
            evt.event,
            Some(steer_proto::agent::v1::session_event::Event::Error(_))
        )));
        assert!(events.iter().any(|evt| matches!(
            evt.event,
            Some(steer_proto::agent::v1::session_event::Event::ProcessingCompleted(_))
        )));
    }

    #[tokio::test]
    async fn test_compaction_flow_end_to_end() {
        let model = steer_core::config::model::builtin::claude_sonnet_4_5();
        let setup = setup_local_grpc_with_stub_provider(model.clone())
            .await
            .expect("local grpc setup");

        let session_id = setup
            .runtime_service
            .handle()
            .create_session(SessionConfig::read_only(
                steer_core::config::model::builtin::claude_sonnet_4_5(),
            ))
            .await
            .expect("create session");

        let mut event_client = AgentServiceClient::new(setup.channel.clone());
        let mut action_client = AgentServiceClient::new(setup.channel.clone());

        let request = tonic::Request::new(SubscribeSessionEventsRequest {
            session_id: session_id.to_string(),
            since_sequence: None,
        });

        let mut stream = event_client
            .subscribe_session_events(request)
            .await
            .expect("subscribe")
            .into_inner();

        let model_spec = crate::grpc::conversions::model_to_proto(model.clone());

        let first_response = action_client
            .send_message(tonic::Request::new(SendMessageRequest {
                session_id: session_id.to_string(),
                message: "first".to_string(),
                model: Some(model_spec.clone()),
            }))
            .await
            .expect("send_message");
        let first_op = first_response.into_inner().operation.expect("operation");
        wait_for_processing_completed(&mut stream, &first_op.id).await;

        let second_response = action_client
            .send_message(tonic::Request::new(SendMessageRequest {
                session_id: session_id.to_string(),
                message: "second".to_string(),
                model: Some(model_spec.clone()),
            }))
            .await
            .expect("send_message");
        let second_op = second_response.into_inner().operation.expect("operation");
        wait_for_processing_completed(&mut stream, &second_op.id).await;

        action_client
            .compact_session(tonic::Request::new(CompactSessionRequest {
                session_id: session_id.to_string(),
                model: Some(model_spec),
            }))
            .await
            .expect("compact_session");

        let mut compact_summary = None;
        let mut compaction_record = None;

        while compact_summary.is_none() || compaction_record.is_none() {
            let event = next_event(&mut stream).await;
            match event.event {
                Some(steer_proto::agent::v1::session_event::Event::CompactResult(e)) => {
                    let result = e.result.expect("compact result");
                    match result.result {
                        Some(steer_proto::agent::v1::compact_result::Result::Success(success)) => {
                            compact_summary = Some(success.summary);
                        }
                        other => panic!("unexpected compact result: {other:?}"),
                    }
                }
                Some(steer_proto::agent::v1::session_event::Event::ConversationCompacted(e)) => {
                    compaction_record = Some(e.record.expect("compaction record"));
                }
                _ => {}
            }
        }

        assert_eq!(compact_summary.expect("summary"), STUB_RESPONSE);
        let record = compaction_record.expect("record");
        assert!(!record.id.is_empty());
        assert_eq!(record.model, model.1);
    }

    #[tokio::test]
    async fn test_send_message_uses_session_default_model_when_not_specified() {
        let setup = setup_local_grpc_with_catalog(
            steer_core::config::model::builtin::claude_sonnet_4_5(),
            None,
            CatalogConfig::default(),
        )
        .await
        .expect("local grpc setup");

        let session_id = setup
            .runtime_service
            .handle()
            .create_session(SessionConfig::read_only(
                steer_core::config::model::builtin::claude_sonnet_4_5(),
            ))
            .await
            .expect("create session");

        let mut client = AgentServiceClient::new(setup.channel.clone());
        let request = tonic::Request::new(SendMessageRequest {
            session_id: session_id.to_string(),
            message: "hello".to_string(),
            model: None,
        });

        let response = client
            .send_message(request)
            .await
            .expect("send_message should succeed using session default model")
            .into_inner();

        assert!(
            response.operation.is_some(),
            "Response should contain an operation"
        );
    }

    #[tokio::test]
    async fn test_compact_session_requires_model_spec() {
        let setup = setup_local_grpc_with_catalog(
            steer_core::config::model::builtin::claude_sonnet_4_5(),
            None,
            CatalogConfig::default(),
        )
        .await
        .expect("local grpc setup");

        let session_id = setup
            .runtime_service
            .handle()
            .create_session(SessionConfig::read_only(
                steer_core::config::model::builtin::claude_sonnet_4_5(),
            ))
            .await
            .expect("create session");

        let mut client = AgentServiceClient::new(setup.channel.clone());
        let request = tonic::Request::new(CompactSessionRequest {
            session_id: session_id.to_string(),
            model: None,
        });

        let err = client
            .compact_session(request)
            .await
            .expect_err("compact_session should fail without model");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_execute_bash_command_does_not_require_model_spec() {
        let setup = setup_local_grpc_with_catalog(
            steer_core::config::model::builtin::claude_sonnet_4_5(),
            None,
            CatalogConfig::default(),
        )
        .await
        .expect("local grpc setup");

        let session_id = setup
            .runtime_service
            .handle()
            .create_session(SessionConfig::read_only(
                steer_core::config::model::builtin::claude_sonnet_4_5(),
            ))
            .await
            .expect("create session");

        let mut client = AgentServiceClient::new(setup.channel.clone());
        let request = tonic::Request::new(ExecuteBashCommandRequest {
            session_id: session_id.to_string(),
            command: "echo hi".to_string(),
        });

        client
            .execute_bash_command(request)
            .await
            .expect("execute_bash_command should succeed without model");
    }
}
