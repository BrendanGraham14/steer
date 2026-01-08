use crate::grpc::conversions::{
    message_to_proto, proto_to_model, proto_to_tool_config, proto_to_workspace_config,
    session_event_to_proto, stream_delta_to_proto,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use steer_core::app::domain::runtime::{RuntimeError, RuntimeHandle};
use steer_core::app::domain::session::{SessionCatalog, SessionFilter};
use steer_core::app::domain::types::SessionId;
use steer_core::session::state::SessionConfig;
use steer_proto::agent::v1::{self as proto, *};
use tokio::sync::{broadcast, mpsc};
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
}

impl RuntimeAgentService {
    pub fn new(
        runtime: RuntimeHandle,
        catalog: Arc<dyn SessionCatalog>,
        llm_config_provider: steer_core::config::LlmConfigProvider,
        model_registry: Arc<steer_core::model_registry::ModelRegistry>,
        provider_registry: Arc<steer_core::auth::ProviderRegistry>,
    ) -> Self {
        Self {
            runtime,
            catalog,
            llm_config_provider,
            model_registry,
            provider_registry,
        }
    }

    fn parse_session_id(session_id: &str) -> Result<SessionId, Status> {
        Uuid::parse_str(session_id)
            .map(SessionId::from)
            .map_err(|_| Status::invalid_argument(format!("Invalid session ID: {session_id}")))
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
        tokio::spawn(async move {
            let mut subscription = subscription;
            while let Some(envelope) = subscription.recv().await {
                if let Some(min_seq) = min_live_seq {
                    if envelope.seq < min_seq {
                        continue;
                    }
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
            debug!("Event forwarding task ended for session: {}", session_id);
        });

        let delta_tx = tx.clone();
        let last_sequence_deltas = last_sequence.clone();
        let delta_sequence_counter = delta_sequence.clone();
        tokio::spawn(async move {
            let mut delta_rx = delta_subscription;
            loop {
                match delta_rx.recv().await {
                    Ok(delta) => {
                        let sequence_num = last_sequence_deltas.load(Ordering::Relaxed);
                        let delta_sequence = delta_sequence_counter.fetch_add(1, Ordering::Relaxed);
                        let proto_event = match stream_delta_to_proto(
                            delta,
                            sequence_num,
                            delta_sequence,
                        ) {
                            Ok(event) => event,
                            Err(e) => {
                                warn!("Failed to convert stream delta: {}", e);
                                continue;
                            }
                        };

                        if let Err(e) = delta_tx.send(Ok(proto_event)).await {
                            warn!("Failed to send delta to client: {}", e);
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("Delta subscription lagged by {} messages", skipped);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            debug!("Delta forwarding task ended for session: {}", session_id);
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let req = request.into_inner();

        let tool_config = req
            .tool_config
            .map(proto_to_tool_config)
            .unwrap_or_default();

        let workspace_config = req
            .workspace_config
            .map(proto_to_workspace_config)
            .unwrap_or_default();

        let session_config = SessionConfig {
            workspace: workspace_config,
            tool_config,
            system_prompt: req.system_prompt,
            metadata: req.metadata,
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
                })),
            };

            for message in state.conversation.messages {
                let proto_msg = message_to_proto(message)
                    .map_err(|e| Status::internal(format!("Failed to convert message: {e}")))?;
                yield GetSessionResponse {
                    chunk: Some(get_session_response::Chunk::Message(proto_msg)),
                };
            }

            yield GetSessionResponse {
                chunk: Some(get_session_response::Chunk::Footer(SessionStateFooter {
                    approved_tools: state.approved_tools.into_iter().collect(),
                    last_event_sequence: state.event_sequence,
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
                state.conversation.messages.len(),
                state.approved_tools.len()
            );

            for msg in state.conversation.messages {
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

        let model_spec = req
            .model
            .ok_or_else(|| Status::invalid_argument("Missing model spec"))?;
        let model = proto_to_model(&model_spec)
            .map_err(|e| Status::invalid_argument(format!("Invalid model spec: {e}")))?;

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

        let model_spec = req
            .model
            .ok_or_else(|| Status::invalid_argument("Missing model spec"))?;
        let model = proto_to_model(&model_spec)
            .map_err(|e| Status::invalid_argument(format!("Invalid model spec: {e}")))?;

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
            .ok_or_else(|| Status::not_found(format!("Session not found: {}", session_id)))?;

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
                auth_schemes: p
                    .auth_schemes
                    .iter()
                    .map(|s| match s {
                        steer_core::config::toml_types::AuthScheme::ApiKey => {
                            proto::ProviderAuthScheme::AuthSchemeApiKey as i32
                        }
                        steer_core::config::toml_types::AuthScheme::Oauth2 => {
                            proto::ProviderAuthScheme::AuthSchemeOauth2 as i32
                        }
                    })
                    .collect(),
            })
            .collect();

        Ok(Response::new(ListProvidersResponse { providers }))
    }

    async fn list_models(
        &self,
        request: Request<ListModelsRequest>,
    ) -> Result<Response<ListModelsResponse>, Status> {
        let req = request.into_inner();

        let all_models: Vec<proto::ProviderModel> = self
            .model_registry
            .recommended()
            .filter(|m| {
                if let Some(ref provider_id) = req.provider_id {
                    m.provider.storage_key() == *provider_id
                } else {
                    true
                }
            })
            .map(|m| proto::ProviderModel {
                provider_id: m.provider.storage_key(),
                model_id: m.id.clone(),
                display_name: m.display_name.clone().unwrap_or_else(|| m.id.clone()),
                supports_thinking: m
                    .parameters
                    .as_ref()
                    .and_then(|p| p.thinking_config.as_ref())
                    .map(|tc| tc.enabled)
                    .unwrap_or(false),
                aliases: m.aliases.clone(),
            })
            .collect();

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
            let status = match self
                .llm_config_provider
                .get_auth_for_provider(&p.id)
                .await
                .map_err(|e| Status::internal(format!("auth lookup failed: {e}")))?
            {
                Some(steer_core::config::ApiAuth::OAuth) => {
                    proto::provider_auth_status::Status::AuthStatusOauth as i32
                }
                Some(steer_core::config::ApiAuth::Key(_)) => {
                    proto::provider_auth_status::Status::AuthStatusApiKey as i32
                }
                None => proto::provider_auth_status::Status::AuthStatusNone as i32,
            };
            statuses.push(proto::ProviderAuthStatus {
                provider_id: p.id.storage_key(),
                status,
            });
        }

        Ok(Response::new(proto::GetProviderAuthStatusResponse {
            statuses,
        }))
    }

    async fn resolve_model(
        &self,
        request: Request<proto::ResolveModelRequest>,
    ) -> Result<Response<proto::ResolveModelResponse>, Status> {
        let req = request.into_inner();

        match self.model_registry.resolve(&req.input) {
            Ok(model_id) => {
                let model_spec = proto::ModelSpec {
                    provider_id: model_id.0.storage_key(),
                    model_id: model_id.1,
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
}
