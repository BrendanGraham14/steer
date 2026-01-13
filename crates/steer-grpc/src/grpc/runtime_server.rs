use crate::grpc::conversions::{
    environment_descriptor_to_proto, message_to_proto, proto_to_model, proto_to_tool_config,
    proto_to_workspace_config, session_event_to_proto, stream_delta_to_proto,
    repo_info_to_proto, workspace_info_to_proto, workspace_status_to_proto,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use steer_core::app::domain::runtime::{RuntimeError, RuntimeHandle};
use steer_core::app::domain::session::{SessionCatalog, SessionFilter};
use steer_core::app::domain::types::SessionId;
use steer_core::auth::{
    AuthFlowWrapper, AuthMethod, AuthSource, DynAuthenticationFlow, ModelId as AuthModelId,
    ModelVisibilityPolicy, ProviderId as AuthProviderId,
};
use steer_core::auth::api_key::ApiKeyAuthFlow;
use steer_core::session::state::SessionConfig;
use steer_proto::agent::v1::{self as proto, *};
use steer_workspace::{EnvironmentManager, RepoManager, WorkspaceManager};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

pub struct RuntimeAgentService {
    runtime: RuntimeHandle,
    catalog: Arc<dyn SessionCatalog>,
    model_registry: Arc<steer_core::model_registry::ModelRegistry>,
    provider_registry: Arc<steer_core::auth::ProviderRegistry>,
    llm_config_provider: steer_core::config::LlmConfigProvider,
    environment_manager: Arc<dyn EnvironmentManager>,
    workspace_manager: Arc<dyn WorkspaceManager>,
    repo_manager: Arc<dyn RepoManager>,
    auth_flow_manager: Arc<AuthFlowManager>,
}

const AUTH_FLOW_TTL: Duration = Duration::from_secs(10 * 60);

struct AuthFlowEntry {
    flow: Arc<dyn DynAuthenticationFlow>,
    state: Box<dyn std::any::Any + Send + Sync>,
    last_updated: Instant,
}

#[derive(Default)]
struct AuthFlowManager {
    flows: Mutex<HashMap<String, AuthFlowEntry>>,
}

impl AuthFlowManager {
    fn new() -> Self {
        Self::default()
    }

    async fn insert(&self, flow_id: String, entry: AuthFlowEntry) {
        let mut flows = self.flows.lock().await;
        flows.insert(flow_id, entry);
    }

    async fn take(&self, flow_id: &str) -> Option<AuthFlowEntry> {
        let mut flows = self.flows.lock().await;
        flows.remove(flow_id)
    }

    async fn cleanup(&self) {
        let mut flows = self.flows.lock().await;
        flows.retain(|_, entry| entry.last_updated.elapsed() <= AUTH_FLOW_TTL);
    }
}

impl RuntimeAgentService {
    pub fn new(
        runtime: RuntimeHandle,
        catalog: Arc<dyn SessionCatalog>,
        llm_config_provider: steer_core::config::LlmConfigProvider,
        model_registry: Arc<steer_core::model_registry::ModelRegistry>,
        provider_registry: Arc<steer_core::auth::ProviderRegistry>,
        environment_manager: Arc<dyn EnvironmentManager>,
        workspace_manager: Arc<dyn WorkspaceManager>,
        repo_manager: Arc<dyn RepoManager>,
    ) -> Self {
        Self {
            runtime,
            catalog,
            llm_config_provider,
            model_registry,
            provider_registry,
            environment_manager,
            workspace_manager,
            repo_manager,
            auth_flow_manager: Arc::new(AuthFlowManager::new()),
        }
    }

    #[allow(clippy::result_large_err)]
    fn parse_session_id(session_id: &str) -> Result<SessionId, Status> {
        Uuid::parse_str(session_id)
            .map(SessionId::from)
            .map_err(|_| Status::invalid_argument(format!("Invalid session ID: {session_id}")))
    }
    fn parse_environment_id(
        environment_id: &str,
    ) -> Result<steer_workspace::EnvironmentId, Status> {
        if environment_id.is_empty() {
            return Ok(steer_workspace::EnvironmentId::local());
        }
        let id = Uuid::parse_str(environment_id).map_err(|_| {
            Status::invalid_argument(format!("Invalid environment ID: {environment_id}"))
        })?;
        Ok(steer_workspace::EnvironmentId::from_uuid(id))
    }

    fn parse_workspace_id(workspace_id: &str) -> Result<steer_workspace::WorkspaceId, Status> {
        let id = Uuid::parse_str(workspace_id).map_err(|_| {
            Status::invalid_argument(format!("Invalid workspace ID: {workspace_id}"))
        })?;
        Ok(steer_workspace::WorkspaceId::from_uuid(id))
    }

    fn parse_repo_id(repo_id: &str) -> Result<steer_workspace::RepoId, Status> {
        let id = Uuid::parse_str(repo_id)
            .map_err(|_| Status::invalid_argument(format!("Invalid repo ID: {repo_id}")))?;
        Ok(steer_workspace::RepoId::from_uuid(id))
    }

    fn proto_to_workspace_ref(
        reference: proto::WorkspaceRef,
    ) -> Result<steer_workspace::WorkspaceRef, Status> {
        let environment_id = Self::parse_environment_id(&reference.environment_id)?;
        let workspace_id = Self::parse_workspace_id(&reference.workspace_id)?;
        let repo_id = Self::parse_repo_id(&reference.repo_id)?;
        Ok(steer_workspace::WorkspaceRef {
            environment_id,
            workspace_id,
            repo_id,
        })
    }

    fn proto_to_repo_ref(reference: proto::RepoRef) -> Result<steer_workspace::RepoRef, Status> {
        let environment_id = Self::parse_environment_id(&reference.environment_id)?;
        let repo_id = Self::parse_repo_id(&reference.repo_id)?;
        let vcs_kind = reference.vcs_kind.and_then(|value| {
            match steer_proto::remote_workspace::v1::VcsKind::try_from(value) {
                Ok(steer_proto::remote_workspace::v1::VcsKind::Git) => {
                    Some(steer_workspace::VcsKind::Git)
                }
                Ok(steer_proto::remote_workspace::v1::VcsKind::Jj) => {
                    Some(steer_workspace::VcsKind::Jj)
                }
                _ => None,
            }
        });
        Ok(steer_workspace::RepoRef {
            environment_id,
            repo_id,
            root_path: std::path::PathBuf::from(reference.root_path),
            vcs_kind,
        })
    }

    fn workspace_manager_error_to_status(err: steer_workspace::WorkspaceManagerError) -> Status {
        match err {
            steer_workspace::WorkspaceManagerError::NotFound(msg) => Status::not_found(msg),
            steer_workspace::WorkspaceManagerError::NotSupported(msg) => {
                Status::failed_precondition(msg)
            }
            steer_workspace::WorkspaceManagerError::InvalidRequest(msg) => {
                Status::invalid_argument(msg)
            }
            steer_workspace::WorkspaceManagerError::Io(msg)
            | steer_workspace::WorkspaceManagerError::Other(msg) => Status::internal(msg),
        }
    }

    fn environment_manager_error_to_status(
        err: steer_workspace::EnvironmentManagerError,
    ) -> Status {
        match err {
            steer_workspace::EnvironmentManagerError::NotFound(msg) => Status::not_found(msg),
            steer_workspace::EnvironmentManagerError::NotSupported(msg) => {
                Status::failed_precondition(msg)
            }
            steer_workspace::EnvironmentManagerError::InvalidRequest(msg) => {
                Status::invalid_argument(msg)
            }
            steer_workspace::EnvironmentManagerError::Io(msg)
            | steer_workspace::EnvironmentManagerError::Other(msg) => Status::internal(msg),
        }
    }

    fn create_auth_flow(
        &self,
        provider_id: &steer_core::config::provider::ProviderId,
    ) -> Result<(Arc<dyn DynAuthenticationFlow>, AuthMethod), Status> {
        let provider_cfg = self.provider_registry.get(provider_id).ok_or_else(|| {
            Status::not_found(format!("Unknown provider: {}", provider_id.as_str()))
        })?;
        let provider_name = provider_cfg.name.clone();
        let auth_storage = self.llm_config_provider.auth_storage().clone();

        if let Some(plugin) = self.llm_config_provider.plugin_registry().get(provider_id)
            && let Some(flow) = plugin.create_flow(auth_storage.clone())
        {
            let methods = flow.available_methods();
            let method = if methods.contains(&AuthMethod::OAuth) {
                AuthMethod::OAuth
            } else if methods.contains(&AuthMethod::ApiKey) {
                AuthMethod::ApiKey
            } else {
                return Err(Status::failed_precondition(format!(
                    "No supported auth methods for provider {}",
                    provider_id.as_str()
                )));
            };
            return Ok((Arc::from(flow), method));
        }

        let flow = AuthFlowWrapper::new(ApiKeyAuthFlow::new(
            auth_storage,
            provider_id.clone(),
            provider_name,
        ));
        Ok((Arc::new(flow), AuthMethod::ApiKey))
    }
}

#[tonic::async_trait]
impl agent_service_server::AgentService for RuntimeAgentService {
    type SubscribeSessionEventsStream = ReceiverStream<Result<SessionEvent, Status>>;
    type ListFilesStream = ReceiverStream<Result<ListFilesResponse, Status>>;
    type GetSessionStream =
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<GetSessionResponse, Status>> + Send>>;
    type GetConversationStream = std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<GetConversationResponse, Status>> + Send>,
    >;

    async fn subscribe_session_events(
        &self,
        request: Request<SubscribeSessionEventsRequest>,
    ) -> Result<Response<Self::SubscribeSessionEventsStream>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;

        if let Err(e) = self.runtime.resume_session(session_id).await
            && !matches!(e, RuntimeError::SessionNotFound { .. })
        {
            error!("Failed to resume session {}: {}", session_id, e);
        }

        let subscription = self
            .runtime
            .subscribe_events(session_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to subscribe: {e}")))?;

        let delta_subscription = self
            .runtime
            .subscribe_deltas(session_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to subscribe to deltas: {e}")))?;

        let (tx, rx) = mpsc::channel(100);
        let last_sequence = Arc::new(AtomicU64::new(req.since_sequence.unwrap_or(0)));
        let delta_sequence = Arc::new(AtomicU64::new(0));

        let mut min_live_seq = req.since_sequence.map(|seq| seq.saturating_add(1));

        if let Some(after_seq) = req.since_sequence {
            match self.runtime.load_events_after(session_id, after_seq).await {
                Ok(events) => {
                    let mut last_seq = after_seq;
                    for (seq, event) in events {
                        last_seq = last_seq.max(seq);
                        let proto_event = match session_event_to_proto(event, seq) {
                            Ok(event) => event,
                            Err(e) => {
                                warn!("Failed to convert session replay event: {}", e);
                                continue;
                            }
                        };

                        if proto_event.event.is_none() {
                            continue;
                        }

                        if let Err(e) = tx.send(Ok(proto_event)).await {
                            warn!("Failed to send replay event to client: {}", e);
                            break;
                        }
                    }
                    min_live_seq = Some(last_seq.saturating_add(1));
                    last_sequence.store(last_seq, Ordering::Relaxed);
                }
                Err(e) => {
                    warn!("Failed to load replay events: {}", e);
                }
            }
        }

        let event_tx = tx.clone();
        let last_sequence_events = last_sequence.clone();
        let delta_sequence_counter = delta_sequence.clone();
        let min_live_seq = min_live_seq;
        tokio::spawn(async move {
            async fn send_delta(
                delta: steer_core::app::domain::delta::StreamDelta,
                tx: &mpsc::Sender<Result<proto::SessionEvent, Status>>,
                last_sequence: &Arc<AtomicU64>,
                delta_sequence: &Arc<AtomicU64>,
            ) -> Result<(), ()> {
                let sequence_num = last_sequence.load(Ordering::Relaxed);
                let delta_sequence = delta_sequence.fetch_add(1, Ordering::Relaxed);
                let proto_event = match stream_delta_to_proto(delta, sequence_num, delta_sequence) {
                    Ok(event) => event,
                    Err(e) => {
                        warn!("Failed to convert stream delta: {}", e);
                        return Ok(());
                    }
                };

                if let Err(e) = tx.send(Ok(proto_event)).await {
                    warn!("Failed to send delta to client: {}", e);
                    return Err(());
                }

                Ok(())
            }

            let mut subscription = subscription;
            let mut delta_rx = delta_subscription;
            let mut events_closed = false;
            let mut deltas_closed = false;

            loop {
                if events_closed && deltas_closed {
                    break;
                }

                tokio::select! {
                    envelope = subscription.recv(), if !events_closed => {
                        match envelope {
                            Some(envelope) => {
                                loop {
                                    match delta_rx.try_recv() {
                                        Ok(delta) => {
                                            if send_delta(
                                                delta,
                                                &event_tx,
                                                &last_sequence_events,
                                                &delta_sequence_counter,
                                            )
                                            .await
                                            .is_err()
                                            {
                                                return;
                                            }
                                        }
                                        Err(broadcast::error::TryRecvError::Empty) => break,
                                        Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                                            warn!("Delta subscription lagged by {} messages", skipped);
                                            continue;
                                        }
                                        Err(broadcast::error::TryRecvError::Closed) => {
                                            deltas_closed = true;
                                            break;
                                        }
                                    }
                                }

                                if let Some(min_seq) = min_live_seq
                                    && envelope.seq < min_seq {
                                        continue;
                                    }

                                let proto_event = match session_event_to_proto(envelope.event, envelope.seq) {
                                    Ok(event) => event,
                                    Err(e) => {
                                        warn!("Failed to convert session event: {}", e);
                                        continue;
                                    }
                                };

                                if proto_event.event.is_none() {
                                    continue;
                                }

                                if let Err(e) = event_tx.send(Ok(proto_event)).await {
                                    warn!("Failed to send event to client: {}", e);
                                    break;
                                }
                                last_sequence_events.store(envelope.seq, Ordering::Relaxed);
                            }
                            None => {
                                events_closed = true;
                            }
                        }
                    }
                    delta = delta_rx.recv(), if !deltas_closed => {
                        match delta {
                            Ok(delta) => {
                                if send_delta(
                                    delta,
                                    &event_tx,
                                    &last_sequence_events,
                                    &delta_sequence_counter,
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                warn!("Delta subscription lagged by {} messages", skipped);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                deltas_closed = true;
                            }
                        }
                    }
                }
            }
            debug!("Event forwarding task ended for session: {}", session_id);
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let req = request.into_inner();

        let default_model_spec = req
            .default_model
            .ok_or_else(|| Status::invalid_argument("Missing required default_model"))?;
        let default_model = proto_to_model(&default_model_spec)
            .map_err(|e| Status::invalid_argument(format!("Invalid default_model: {e}")))?;

        let tool_config = req
            .tool_config
            .map(proto_to_tool_config)
            .unwrap_or_default();

        let workspace_config = req
            .workspace_config
            .map(proto_to_workspace_config)
            .unwrap_or_default();

        let workspace_id = match req.workspace_id {
            Some(value) => Some(Self::parse_workspace_id(&value)?),
            None => None,
        };

        let workspace_ref = match req.workspace_ref {
            Some(reference) => Some(Self::proto_to_workspace_ref(reference)?),
            None => None,
        };

        let mut repo_ref = match req.repo_ref {
            Some(reference) => Some(Self::proto_to_repo_ref(reference)?),
            None => None,
        };

        let parent_session_id = match req.parent_session_id {
            Some(value) => Some(Self::parse_session_id(&value)?),
            None => None,
        };

        if repo_ref.is_none()
            && let steer_core::session::state::WorkspaceConfig::Local { path } = &workspace_config
        {
            match self
                .repo_manager
                .resolve_repo(steer_workspace::EnvironmentId::local(), path)
                .await
            {
                Ok(repo_info) => {
                    repo_ref = Some(steer_workspace::RepoRef {
                        environment_id: repo_info.environment_id,
                        repo_id: repo_info.repo_id,
                        root_path: repo_info.root_path.clone(),
                        vcs_kind: repo_info.vcs_kind,
                    });
                }
                Err(_) => {}
            }
        }

        let session_config = SessionConfig {
            workspace: workspace_config,
            workspace_ref,
            workspace_id,
            repo_ref,
            parent_session_id,
            workspace_name: req.workspace_name,
            tool_config,
            system_prompt: req.system_prompt,
            metadata: req.metadata,
            default_model,
        };

        match self.runtime.create_session(session_config.clone()).await {
            Ok(session_id) => {
                if let Err(e) = self
                    .catalog
                    .update_session_catalog(session_id, Some(&session_config), false, None)
                    .await
                {
                    error!("Failed to update session catalog: {}", e);
                    return Err(Status::internal(format!(
                        "Failed to update session catalog: {e}"
                    )));
                }

                let session_info = SessionInfo {
                    id: session_id.to_string(),
                    created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                    updated_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                    status: proto::SessionStatus::Active as i32,
                    metadata: None,
                };
                Ok(Response::new(CreateSessionResponse {
                    session: Some(session_info),
                }))
            }
            Err(e) => {
                error!("Failed to create session: {}", e);
                Err(Status::internal(format!("Failed to create session: {e}")))
            }
        }
    }

    async fn list_sessions(
        &self,
        _request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let filter = SessionFilter::default();

        match self.catalog.list_sessions(filter).await {
            Ok(sessions) => {
                let proto_sessions = sessions
                    .into_iter()
                    .map(|s| SessionInfo {
                        id: s.id.to_string(),
                        created_at: Some(prost_types::Timestamp::from(
                            std::time::SystemTime::from(s.created_at),
                        )),
                        updated_at: Some(prost_types::Timestamp::from(
                            std::time::SystemTime::from(s.updated_at),
                        )),
                        status: proto::SessionStatus::Active as i32,
                        metadata: None,
                    })
                    .collect();

                Ok(Response::new(ListSessionsResponse {
                    sessions: proto_sessions,
                    next_page_token: None,
                }))
            }
            Err(e) => {
                error!("Failed to list sessions: {}", e);
                Err(Status::internal(format!("Failed to list sessions: {e}")))
            }
        }
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<Self::GetSessionStream>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;
        let runtime = self.runtime.clone();
        let catalog = self.catalog.clone();

        let stream = async_stream::try_stream! {
            if let Err(e) = runtime.resume_session(session_id).await
                && matches!(e, RuntimeError::SessionNotFound { .. }) {
                    Err(Status::not_found(format!("Session not found: {session_id}")))?;
                    return;
                }

            let state = runtime.get_session_state(session_id).await
                .map_err(|e| Status::internal(format!("Failed to get session state: {e}")))?;

            let config = catalog.get_session_config(session_id).await
                .map_err(|e| Status::internal(format!("Failed to get session config: {e}")))?;

            yield GetSessionResponse {
                chunk: Some(get_session_response::Chunk::Header(SessionStateHeader {
                    id: session_id.to_string(),
                    created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                    updated_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                    config: config.map(|c| crate::grpc::conversions::session_config_to_proto(&c)),
                    last_event_sequence: state.event_sequence,
                })),
            };

            for message in state.message_graph.messages {
                let proto_msg = message_to_proto(message)
                    .map_err(|e| Status::internal(format!("Failed to convert message: {e}")))?;
                yield GetSessionResponse {
                    chunk: Some(get_session_response::Chunk::Message(proto_msg)),
                };
            }

            yield GetSessionResponse {
                chunk: Some(get_session_response::Chunk::Footer(SessionStateFooter {
                    approved_tools: state.approved_tools.into_iter().collect(),
                    metadata: std::collections::HashMap::new(),
                })),
            };
        };

        Ok(Response::new(Box::pin(stream)))
    }

    async fn delete_session(
        &self,
        request: Request<DeleteSessionRequest>,
    ) -> Result<Response<DeleteSessionResponse>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;

        match self.runtime.delete_session(session_id).await {
            Ok(()) => Ok(Response::new(DeleteSessionResponse {})),
            Err(RuntimeError::SessionNotFound { .. }) => Err(Status::not_found(format!(
                "Session not found: {}",
                req.session_id
            ))),
            Err(e) => {
                error!("Failed to delete session: {}", e);
                Err(Status::internal(format!("Failed to delete session: {e}")))
            }
        }
    }

    async fn get_conversation(
        &self,
        request: Request<GetConversationRequest>,
    ) -> Result<Response<Self::GetConversationStream>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;
        let runtime = self.runtime.clone();

        info!("GetConversation called for session: {}", session_id);

        let stream = async_stream::try_stream! {
            if let Err(e) = runtime.resume_session(session_id).await
                && matches!(e, RuntimeError::SessionNotFound { .. }) {
                    Err(Status::not_found(format!("Session not found: {session_id}")))?;
                    return;
                }

            let state = runtime.get_session_state(session_id).await
                .map_err(|e| Status::internal(format!("Failed to get session state: {e}")))?;

            info!(
                "Found session state with {} messages and {} approved tools",
                state.message_graph.messages.len(),
                state.approved_tools.len()
            );

            for msg in state.message_graph.messages {
                let proto_msg = message_to_proto(msg)
                    .map_err(|e| Status::internal(format!("Failed to convert message: {e}")))?;
                yield GetConversationResponse {
                    chunk: Some(get_conversation_response::Chunk::Message(proto_msg)),
                };
            }

            yield GetConversationResponse {
                chunk: Some(get_conversation_response::Chunk::Footer(GetConversationFooter {
                    approved_tools: state.approved_tools.into_iter().collect(),
                })),
            };
        };

        Ok(Response::new(Box::pin(stream)))
    }

    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<SendMessageResponse>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;

        let model = if let Some(model_spec) = req.model {
            proto_to_model(&model_spec)
                .map_err(|e| Status::invalid_argument(format!("Invalid model spec: {e}")))?
        } else {
            let config = self
                .catalog
                .get_session_config(session_id)
                .await
                .map_err(|e| Status::internal(format!("Failed to get session config: {e}")))?
                .ok_or_else(|| Status::not_found("Session config not found"))?;
            config.default_model
        };

        match self
            .runtime
            .submit_user_input(session_id, req.message, model)
            .await
        {
            Ok(op_id) => Ok(Response::new(SendMessageResponse {
                operation: Some(Operation {
                    id: op_id.to_string(),
                    session_id: session_id.to_string(),
                    r#type: OperationType::SendMessage as i32,
                    status: OperationStatus::Running as i32,
                    created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                    completed_at: None,
                    metadata: std::collections::HashMap::new(),
                }),
            })),
            Err(e) => {
                error!("Failed to send message: {}", e);
                Err(Status::internal(format!("Failed to send message: {e}")))
            }
        }
    }

    async fn edit_message(
        &self,
        request: Request<EditMessageRequest>,
    ) -> Result<Response<EditMessageResponse>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;

        let model = if let Some(model_spec) = req.model {
            proto_to_model(&model_spec)
                .map_err(|e| Status::invalid_argument(format!("Invalid model spec: {e}")))?
        } else {
            let config = self
                .catalog
                .get_session_config(session_id)
                .await
                .map_err(|e| Status::internal(format!("Failed to get session config: {e}")))?
                .ok_or_else(|| Status::not_found("Session config not found"))?;
            config.default_model
        };

        self.runtime
            .submit_edited_message(session_id, req.message_id, req.new_content, model)
            .await
            .map_err(|e| Status::internal(format!("Failed to edit message: {e}")))?;

        Ok(Response::new(EditMessageResponse {}))
    }

    async fn approve_tool(
        &self,
        request: Request<ApproveToolRequest>,
    ) -> Result<Response<ApproveToolResponse>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;

        let request_id = Uuid::parse_str(&req.tool_call_id)
            .map(steer_core::app::domain::types::RequestId::from)
            .map_err(|_| Status::invalid_argument("Invalid tool call ID"))?;

        let (approved, remember) = match req.decision {
            Some(decision) => match decision.decision_type {
                Some(proto::approval_decision::DecisionType::Deny(_)) => (false, None),
                Some(proto::approval_decision::DecisionType::Once(_)) => (true, None),
                Some(proto::approval_decision::DecisionType::AlwaysTool(_)) => (
                    true,
                    Some(steer_core::app::domain::action::ApprovalMemory::PendingTool),
                ),
                Some(proto::approval_decision::DecisionType::AlwaysBashPattern(pattern)) => (
                    true,
                    Some(steer_core::app::domain::action::ApprovalMemory::BashPattern(pattern)),
                ),
                None => {
                    return Err(Status::invalid_argument("Invalid approval decision"));
                }
            },
            None => {
                return Err(Status::invalid_argument("Missing approval decision"));
            }
        };

        match self
            .runtime
            .submit_tool_approval(session_id, request_id, approved, remember)
            .await
        {
            Ok(()) => Ok(Response::new(ApproveToolResponse {})),
            Err(e) => {
                error!("Failed to approve tool: {}", e);
                Err(Status::internal(format!("Failed to approve tool: {e}")))
            }
        }
    }

    async fn cancel_operation(
        &self,
        request: Request<CancelOperationRequest>,
    ) -> Result<Response<CancelOperationResponse>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;

        match self.runtime.cancel_operation(session_id, None).await {
            Ok(()) => Ok(Response::new(CancelOperationResponse {})),
            Err(e) => {
                error!("Failed to cancel operation: {}", e);
                Err(Status::internal(format!("Failed to cancel operation: {e}")))
            }
        }
    }

    async fn compact_session(
        &self,
        request: Request<CompactSessionRequest>,
    ) -> Result<Response<CompactSessionResponse>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;
        let model_spec = req
            .model
            .ok_or_else(|| Status::invalid_argument("Missing model spec"))?;
        let model = proto_to_model(&model_spec)
            .map_err(|e| Status::invalid_argument(format!("Invalid model spec: {e}")))?;

        self.runtime
            .compact_session(session_id, model)
            .await
            .map_err(|e| Status::internal(format!("Failed to compact session: {e}")))?;

        Ok(Response::new(CompactSessionResponse {}))
    }

    async fn execute_bash_command(
        &self,
        request: Request<ExecuteBashCommandRequest>,
    ) -> Result<Response<ExecuteBashCommandResponse>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;

        self.runtime
            .execute_bash_command(session_id, req.command)
            .await
            .map_err(|e| Status::internal(format!("Failed to execute bash command: {e}")))?;

        Ok(Response::new(ExecuteBashCommandResponse {}))
    }

    async fn list_files(
        &self,
        request: Request<ListFilesRequest>,
    ) -> Result<Response<Self::ListFilesStream>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;

        debug!("ListFiles called for session: {}", session_id);

        let config = self
            .catalog
            .get_session_config(session_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to get session config: {e}")))?
            .ok_or_else(|| Status::not_found(format!("Session not found: {session_id}")))?;

        let workspace =
            steer_core::workspace::create_workspace(&config.workspace.to_workspace_config())
                .await
                .map_err(|e| Status::internal(format!("Failed to create workspace: {e}")))?;

        let (tx, rx) = mpsc::channel(100);

        let _list_task: tokio::task::JoinHandle<()> = tokio::spawn(async move {
            let query = if req.query.is_empty() {
                None
            } else {
                Some(req.query.as_str())
            };

            let max_results = if req.max_results == 0 {
                None
            } else {
                Some(req.max_results as usize)
            };

            match workspace.list_files(query, max_results).await {
                Ok(files) => {
                    for chunk in files.chunks(1000) {
                        let response = ListFilesResponse {
                            paths: chunk.to_vec(),
                        };

                        if let Err(e) = tx.send(Ok(response)).await {
                            warn!("Failed to send file list chunk: {}", e);
                            break;
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to list files: {}", e);
                    let _ = tx
                        .send(Err(Status::internal(format!("Failed to list files: {e}"))))
                        .await;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_mcp_servers(
        &self,
        request: Request<GetMcpServersRequest>,
    ) -> Result<Response<GetMcpServersResponse>, Status> {
        let req = request.into_inner();
        let session_id = Self::parse_session_id(&req.session_id)?;

        debug!("GetMcpServers called for session: {}", session_id);

        let state = self
            .runtime
            .get_session_state(session_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to get session state: {e}")))?;

        let config = self
            .catalog
            .get_session_config(session_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to get session config: {e}")))?;

        let transport_map: std::collections::HashMap<String, &steer_core::tools::McpTransport> =
            config
                .as_ref()
                .map(|c| {
                    c.tool_config
                        .backends
                        .iter()
                        .map(|b| {
                            let steer_core::session::state::BackendConfig::Mcp {
                                server_name,
                                transport,
                                ..
                            } = b;
                            (server_name.clone(), transport)
                        })
                        .collect()
                })
                .unwrap_or_default();

        let servers: Vec<proto::McpServerInfo> = state
            .mcp_servers
            .into_iter()
            .map(|(name, mcp_state)| {
                use crate::grpc::conversions::mcp_transport_to_proto;
                use steer_core::app::domain::action::McpServerState;

                let state = match mcp_state {
                    McpServerState::Connecting => proto::McpConnectionState {
                        state: Some(proto::mcp_connection_state::State::Connecting(
                            proto::McpConnecting {},
                        )),
                    },
                    McpServerState::Connected { tools } => {
                        let tool_names = tools.iter().map(|t| t.name.clone()).collect();
                        proto::McpConnectionState {
                            state: Some(proto::mcp_connection_state::State::Connected(
                                proto::McpConnected { tool_names },
                            )),
                        }
                    }
                    McpServerState::Disconnected { error } => {
                        let error_msg = error.unwrap_or_else(|| "Disconnected".to_string());
                        proto::McpConnectionState {
                            state: Some(proto::mcp_connection_state::State::Failed(
                                proto::McpFailed { error: error_msg },
                            )),
                        }
                    }
                    McpServerState::Failed { error } => proto::McpConnectionState {
                        state: Some(proto::mcp_connection_state::State::Failed(
                            proto::McpFailed { error },
                        )),
                    },
                };

                proto::McpServerInfo {
                    server_name: name.clone(),
                    transport: transport_map.get(&name).map(|t| mcp_transport_to_proto(t)),
                    state: Some(state),
                    last_updated: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                }
            })
            .collect();

        Ok(Response::new(GetMcpServersResponse { servers }))
    }

    async fn list_providers(
        &self,
        _request: Request<ListProvidersRequest>,
    ) -> Result<Response<ListProvidersResponse>, Status> {
        let providers = self
            .provider_registry
            .all()
            .map(|p| proto::ProviderInfo {
                id: p.id.storage_key(),
                name: p.name.clone(),
            })
            .collect();

        Ok(Response::new(ListProvidersResponse { providers }))
    }

    async fn list_models(
        &self,
        request: Request<ListModelsRequest>,
    ) -> Result<Response<ListModelsResponse>, Status> {
        let req = request.into_inner();

        let mut auth_sources: HashMap<steer_core::config::provider::ProviderId, AuthSource> =
            HashMap::new();
        let mut visibility_policies: HashMap<
            steer_core::config::provider::ProviderId,
            Option<Arc<dyn ModelVisibilityPolicy>>,
        > = HashMap::new();

        let mut all_models = Vec::new();

        for model in self.model_registry.recommended() {
            if let Some(ref provider_id) = req.provider_id {
                if model.provider.storage_key() != *provider_id {
                    continue;
                }
            }

            let provider_id = model.provider.clone();

            let auth_source = if let Some(source) = auth_sources.get(&provider_id) {
                source.clone()
            } else {
                let source = match self
                    .llm_config_provider
                    .resolve_auth_source(&provider_id)
                    .await
                {
                    Ok(source) => source,
                    Err(err) => {
                        warn!(
                            "Failed to resolve auth source for provider {}: {err}",
                            provider_id.as_str()
                        );
                        AuthSource::None
                    }
                };
                auth_sources.insert(provider_id.clone(), source.clone());
                source
            };

            let policy = visibility_policies
                .entry(provider_id.clone())
                .or_insert_with(|| {
                    self.llm_config_provider
                        .plugin_registry()
                        .get(&provider_id)
                        .and_then(|plugin| plugin.model_visibility().map(Arc::from))
                });

            if let Some(policy) = policy {
                let auth_model_id = AuthModelId {
                    provider_id: AuthProviderId(provider_id.as_str().to_string()),
                    model_id: model.id.clone(),
                };
                if !policy.allow_model(&auth_model_id, &auth_source) {
                    continue;
                }
            }

            all_models.push(proto::ProviderModel {
                provider_id: model.provider.storage_key(),
                model_id: model.id.clone(),
                display_name: model
                    .display_name
                    .clone()
                    .unwrap_or_else(|| model.id.clone()),
                supports_thinking: model
                    .parameters
                    .as_ref()
                    .and_then(|p| p.thinking_config.as_ref())
                    .map(|tc| tc.enabled)
                    .unwrap_or(false),
                aliases: model.aliases.clone(),
            });
        }

        Ok(Response::new(ListModelsResponse { models: all_models }))
    }

    async fn get_provider_auth_status(
        &self,
        request: Request<proto::GetProviderAuthStatusRequest>,
    ) -> Result<Response<proto::GetProviderAuthStatusResponse>, Status> {
        let req = request.into_inner();

        let mut statuses = Vec::new();
        for p in self.provider_registry.all() {
            if let Some(ref filter) = req.provider_id
                && &p.id.storage_key() != filter
            {
                continue;
            }
            let auth_source = self
                .llm_config_provider
                .resolve_auth_source(&p.id)
                .await
                .map_err(|e| Status::internal(format!("auth lookup failed: {e}")))?;
            let auth_source = crate::grpc::conversions::auth_source_to_proto(auth_source);
            statuses.push(proto::ProviderAuthStatus {
                provider_id: p.id.storage_key(),
                auth_source: Some(auth_source),
            });
        }

        Ok(Response::new(proto::GetProviderAuthStatusResponse {
            statuses,
        }))
    }

    async fn start_auth(
        &self,
        request: Request<proto::StartAuthRequest>,
    ) -> Result<Response<proto::StartAuthResponse>, Status> {
        self.auth_flow_manager.cleanup().await;
        let req = request.into_inner();
        let provider_id = steer_core::config::provider::ProviderId(req.provider_id);

        let (flow, method) = self.create_auth_flow(&provider_id)?;
        let state = flow
            .start_auth(method)
            .await
            .map_err(|e| Status::internal(format!("auth start failed: {e}")))?;
        let progress = flow
            .get_initial_progress(&state, method)
            .await
            .map_err(|e| Status::internal(format!("auth progress failed: {e}")))?;

        let flow_id = Uuid::new_v4().to_string();
        self.auth_flow_manager
            .insert(
                flow_id.clone(),
                AuthFlowEntry {
                    flow,
                    state,
                    last_updated: Instant::now(),
                },
            )
            .await;

        Ok(Response::new(proto::StartAuthResponse {
            flow_id,
            progress: Some(crate::grpc::conversions::auth_progress_to_proto(progress)),
        }))
    }

    async fn send_auth_input(
        &self,
        request: Request<proto::SendAuthInputRequest>,
    ) -> Result<Response<proto::SendAuthInputResponse>, Status> {
        self.auth_flow_manager.cleanup().await;
        let req = request.into_inner();
        let flow_id = req.flow_id.clone();

        let mut entry = self
            .auth_flow_manager
            .take(&flow_id)
            .await
            .ok_or_else(|| Status::not_found("Auth flow not found"))?;

        let progress = entry
            .flow
            .handle_input(&mut entry.state, &req.input)
            .await
            .map_err(|e| Status::internal(format!("auth input failed: {e}")))?;

        let done = matches!(
            progress,
            steer_core::auth::AuthProgress::Complete | steer_core::auth::AuthProgress::Error(_)
        );

        if !done {
            entry.last_updated = Instant::now();
            self.auth_flow_manager.insert(flow_id, entry).await;
        }

        Ok(Response::new(proto::SendAuthInputResponse {
            progress: Some(crate::grpc::conversions::auth_progress_to_proto(progress)),
        }))
    }

    async fn get_auth_progress(
        &self,
        request: Request<proto::GetAuthProgressRequest>,
    ) -> Result<Response<proto::GetAuthProgressResponse>, Status> {
        self.auth_flow_manager.cleanup().await;
        let req = request.into_inner();
        let flow_id = req.flow_id.clone();

        let mut entry = self
            .auth_flow_manager
            .take(&flow_id)
            .await
            .ok_or_else(|| Status::not_found("Auth flow not found"))?;

        let progress = entry
            .flow
            .handle_input(&mut entry.state, "")
            .await
            .map_err(|e| Status::internal(format!("auth progress failed: {e}")))?;

        let done = matches!(
            progress,
            steer_core::auth::AuthProgress::Complete | steer_core::auth::AuthProgress::Error(_)
        );

        if !done {
            entry.last_updated = Instant::now();
            self.auth_flow_manager.insert(flow_id, entry).await;
        }

        Ok(Response::new(proto::GetAuthProgressResponse {
            progress: Some(crate::grpc::conversions::auth_progress_to_proto(progress)),
        }))
    }

    async fn cancel_auth(
        &self,
        request: Request<proto::CancelAuthRequest>,
    ) -> Result<Response<proto::CancelAuthResponse>, Status> {
        self.auth_flow_manager.cleanup().await;
        let req = request.into_inner();
        let flow_id = req.flow_id;

        let _ = self.auth_flow_manager.take(&flow_id).await;

        Ok(Response::new(proto::CancelAuthResponse {}))
    }

    async fn resolve_model(
        &self,
        request: Request<proto::ResolveModelRequest>,
    ) -> Result<Response<proto::ResolveModelResponse>, Status> {
        let req = request.into_inner();

        match self.model_registry.resolve(&req.input) {
            Ok(model_id) => {
                let steer_core::config::model::ModelId { provider, id } = model_id;
                let model_spec = proto::ModelSpec {
                    provider_id: provider.storage_key(),
                    model_id: id,
                };
                Ok(Response::new(proto::ResolveModelResponse {
                    model: Some(model_spec),
                }))
            }
            Err(e) => Err(Status::not_found(format!(
                "Failed to resolve model '{}': {}",
                req.input, e
            ))),
        }
    }

    async fn create_workspace(
        &self,
        request: Request<proto::CreateWorkspaceRequest>,
    ) -> Result<Response<proto::CreateWorkspaceResponse>, Status> {
        let req = request.into_inner();
        let repo_id = Self::parse_repo_id(&req.repo_id)?;
        let parent_workspace_id = match req.parent_workspace_id {
            Some(value) => Some(Self::parse_workspace_id(&value)?),
            None => None,
        };

        let strategy = match proto::WorkspaceCreateStrategy::try_from(req.strategy) {
            Ok(proto::WorkspaceCreateStrategy::JjWorkspace) => {
                steer_workspace::WorkspaceCreateStrategy::JjWorkspace
            }
            Ok(proto::WorkspaceCreateStrategy::GitWorktree) => {
                steer_workspace::WorkspaceCreateStrategy::GitWorktree
            }
            _ => {
                return Err(Status::invalid_argument(
                    "Unsupported workspace create strategy",
                ));
            }
        };

        let request = steer_workspace::CreateWorkspaceRequest {
            repo_id,
            name: req.name,
            parent_workspace_id,
            strategy,
        };

        let workspace = self
            .workspace_manager
            .create_workspace(request)
            .await
            .map_err(Self::workspace_manager_error_to_status)?;

        Ok(Response::new(proto::CreateWorkspaceResponse {
            workspace: Some(workspace_info_to_proto(&workspace)),
        }))
    }

    async fn resolve_repo(
        &self,
        request: Request<proto::ResolveRepoRequest>,
    ) -> Result<Response<proto::ResolveRepoResponse>, Status> {
        let req = request.into_inner();
        let environment_id = Self::parse_environment_id(&req.environment_id)?;
        let repo = self
            .repo_manager
            .resolve_repo(environment_id, std::path::Path::new(&req.path))
            .await
            .map_err(Self::workspace_manager_error_to_status)?;

        Ok(Response::new(proto::ResolveRepoResponse {
            repo: Some(repo_info_to_proto(&repo)),
        }))
    }

    async fn list_repos(
        &self,
        request: Request<proto::ListReposRequest>,
    ) -> Result<Response<proto::ListReposResponse>, Status> {
        let req = request.into_inner();
        let environment_id = Self::parse_environment_id(&req.environment_id)?;
        let repos = self
            .repo_manager
            .list_repos(environment_id)
            .await
            .map_err(Self::workspace_manager_error_to_status)?;

        Ok(Response::new(proto::ListReposResponse {
            repos: repos.iter().map(repo_info_to_proto).collect(),
        }))
    }

    async fn list_workspaces(
        &self,
        request: Request<proto::ListWorkspacesRequest>,
    ) -> Result<Response<proto::ListWorkspacesResponse>, Status> {
        let req = request.into_inner();
        let environment_id = Self::parse_environment_id(&req.environment_id)?;

        let workspaces = self
            .workspace_manager
            .list_workspaces(steer_workspace::ListWorkspacesRequest { environment_id })
            .await
            .map_err(Self::workspace_manager_error_to_status)?;

        Ok(Response::new(proto::ListWorkspacesResponse {
            workspaces: workspaces
                .iter()
                .map(workspace_info_to_proto)
                .collect(),
        }))
    }

    async fn get_workspace_status(
        &self,
        request: Request<proto::GetWorkspaceStatusRequest>,
    ) -> Result<Response<proto::GetWorkspaceStatusResponse>, Status> {
        let req = request.into_inner();
        let workspace_id = Self::parse_workspace_id(&req.workspace_id)?;

        let status = self
            .workspace_manager
            .get_workspace_status(workspace_id)
            .await
            .map_err(Self::workspace_manager_error_to_status)?;

        Ok(Response::new(proto::GetWorkspaceStatusResponse {
            status: Some(workspace_status_to_proto(&status)),
        }))
    }

    async fn delete_workspace(
        &self,
        request: Request<proto::DeleteWorkspaceRequest>,
    ) -> Result<Response<proto::DeleteWorkspaceResponse>, Status> {
        let req = request.into_inner();
        let workspace_id = Self::parse_workspace_id(&req.workspace_id)?;

        self.workspace_manager
            .delete_workspace(steer_workspace::DeleteWorkspaceRequest { workspace_id })
            .await
            .map_err(Self::workspace_manager_error_to_status)?;

        Ok(Response::new(proto::DeleteWorkspaceResponse {}))
    }

    async fn create_environment(
        &self,
        request: Request<proto::CreateEnvironmentRequest>,
    ) -> Result<Response<proto::CreateEnvironmentResponse>, Status> {
        let req = request.into_inner();
        let request = steer_workspace::CreateEnvironmentRequest {
            root: req.root_path.map(std::path::PathBuf::from),
            name: req.name,
        };

        let env = self
            .environment_manager
            .create_environment(request)
            .await
            .map_err(Self::environment_manager_error_to_status)?;

        Ok(Response::new(proto::CreateEnvironmentResponse {
            environment: Some(environment_descriptor_to_proto(&env)),
        }))
    }

    async fn get_environment(
        &self,
        request: Request<proto::GetEnvironmentRequest>,
    ) -> Result<Response<proto::GetEnvironmentResponse>, Status> {
        let req = request.into_inner();
        let environment_id = Self::parse_environment_id(&req.environment_id)?;

        let env = self
            .environment_manager
            .get_environment(environment_id)
            .await
            .map_err(Self::environment_manager_error_to_status)?;

        Ok(Response::new(proto::GetEnvironmentResponse {
            environment: Some(environment_descriptor_to_proto(&env)),
        }))
    }

    async fn delete_environment(
        &self,
        request: Request<proto::DeleteEnvironmentRequest>,
    ) -> Result<Response<proto::DeleteEnvironmentResponse>, Status> {
        let req = request.into_inner();
        let environment_id = Self::parse_environment_id(&req.environment_id)?;
        let policy = match proto::EnvironmentDeletePolicy::try_from(req.policy) {
            Ok(proto::EnvironmentDeletePolicy::Soft) => {
                steer_workspace::EnvironmentDeletePolicy::Soft
            }
            Ok(proto::EnvironmentDeletePolicy::Hard) => {
                steer_workspace::EnvironmentDeletePolicy::Hard
            }
            _ => steer_workspace::EnvironmentDeletePolicy::Hard,
        };

        self.environment_manager
            .delete_environment(environment_id, policy)
            .await
            .map_err(Self::environment_manager_error_to_status)?;

        Ok(Response::new(proto::DeleteEnvironmentResponse {}))
    }
}
