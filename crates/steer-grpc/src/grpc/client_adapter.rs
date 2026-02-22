use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tonic::Request;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use crate::client_api::{
    ClientEvent, CreateSessionParams, PrimaryAgentSpec, ProviderAuthStatus, ProviderInfo,
    StartAuthResponse,
};
use crate::grpc::conversions::{
    model_to_proto, proto_to_client_event, proto_to_mcp_server_info, proto_to_message,
    proto_to_primary_agent_spec, proto_to_provider_auth_status, proto_to_provider_info,
    proto_to_repo_info, proto_to_start_auth_response, proto_to_workspace_info,
    proto_to_workspace_status, session_policy_overrides_to_proto, session_tool_config_to_proto,
    workspace_config_to_proto,
};
use crate::grpc::error::GrpcError;

type GrpcResult<T> = std::result::Result<T, GrpcError>;

use steer_core::app::conversation::Message;
use steer_core::session::McpServerInfo;
use steer_proto::agent::v1::{
    self as proto, CreateSessionRequest, DeleteSessionRequest, GetConversationRequest,
    GetDefaultModelRequest, GetMcpServersRequest, GetSessionRequest, GetWorkspaceStatusRequest,
    ListReposRequest, ListSessionsRequest, ListWorkspacesRequest, ResolveRepoRequest, SessionInfo,
    SessionState, agent_service_client::AgentServiceClient,
};

pub struct AgentClient {
    client: Mutex<AgentServiceClient<Channel>>,
    session_id: Mutex<Option<String>>,
    client_event_tx: mpsc::Sender<ClientEvent>,
    client_event_rx: Mutex<Option<mpsc::Receiver<ClientEvent>>>,
    stream_handle: Mutex<Option<JoinHandle<()>>>,
}

impl AgentClient {
    pub async fn connect(addr: &str) -> GrpcResult<Self> {
        info!("Connecting to gRPC server at {}", addr);

        let client = AgentServiceClient::connect(addr.to_string()).await?;

        info!("Successfully connected to gRPC server");

        let (client_event_tx, client_event_rx) = mpsc::channel::<ClientEvent>(100);

        Ok(Self {
            client: Mutex::new(client),
            session_id: Mutex::new(None),
            client_event_tx,
            client_event_rx: Mutex::new(Some(client_event_rx)),
            stream_handle: Mutex::new(None),
        })
    }

    pub async fn from_channel(channel: Channel) -> GrpcResult<Self> {
        info!("Creating gRPC client from provided channel");

        let client = AgentServiceClient::new(channel);
        let (client_event_tx, client_event_rx) = mpsc::channel::<ClientEvent>(100);

        Ok(Self {
            client: Mutex::new(client),
            session_id: Mutex::new(None),
            client_event_tx,
            client_event_rx: Mutex::new(Some(client_event_rx)),
            stream_handle: Mutex::new(None),
        })
    }

    pub async fn local(default_model: steer_core::config::model::ModelId) -> GrpcResult<Self> {
        use crate::local_server::setup_local_grpc;
        let (channel, _server_handle) = setup_local_grpc(default_model, None, None).await?;
        Self::from_channel(channel).await
    }

    pub async fn create_session(&self, params: CreateSessionParams) -> GrpcResult<String> {
        debug!("Creating new session with gRPC server");

        let workspace_config = workspace_config_to_proto(&params.workspace);
        let tool_config = session_tool_config_to_proto(&params.tool_config);

        let request = Request::new(CreateSessionRequest {
            metadata: params.metadata,
            tool_config: Some(tool_config),
            workspace_config: Some(workspace_config),
            default_model: Some(model_to_proto(params.default_model)),
            primary_agent_id: params.primary_agent_id,
            policy_overrides: Some(session_policy_overrides_to_proto(&params.policy_overrides)),
            auto_compaction: None,
        });

        let response = self
            .client
            .lock()
            .await
            .create_session(request)
            .await
            .map_err(Box::new)?;
        let response = response.into_inner();
        let session = response
            .session
            .ok_or_else(|| Box::new(tonic::Status::internal("No session info in response")))?;

        *self.session_id.lock().await = Some(session.id.clone());

        info!("Created session: {}", session.id);
        Ok(session.id)
    }

    pub async fn resume_session(
        &self,
        session_id: &str,
    ) -> GrpcResult<(Vec<Message>, Vec<String>, Vec<String>)> {
        let result = self.get_conversation(session_id).await?;
        *self.session_id.lock().await = Some(session_id.to_string());
        info!("Resumed session: {}", session_id);
        Ok(result)
    }

    pub async fn subscribe_session_events(&self) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session - call create_session or resume_session first"
                    .to_string(),
            })?;

        debug!("Subscribing to session events for session: {}", session_id);

        if let Some(handle) = self.stream_handle.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }

        let evt_tx = self.client_event_tx.clone();

        let request = Request::new(proto::SubscribeSessionEventsRequest {
            session_id: session_id.clone(),
            since_sequence: None,
        });

        let mut inbound_stream = self
            .client
            .lock()
            .await
            .subscribe_session_events(request)
            .await
            .map_err(Box::new)?
            .into_inner();

        let session_id_clone = session_id.clone();
        let stream_handle = tokio::spawn(async move {
            info!(
                "Started event subscription handler for session: {}",
                session_id_clone
            );

            while let Some(result) = inbound_stream.message().await.transpose() {
                match result {
                    Ok(server_event) => match proto_to_client_event(server_event) {
                        Ok(Some(client_event)) => {
                            if let Err(e) = evt_tx.send(client_event).await {
                                warn!("Failed to forward client event: {}", e);
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            error!("Failed to convert server event: {}", e);
                        }
                    },
                    Err(e) => {
                        error!("gRPC stream error: {}", e);
                        break;
                    }
                }
            }

            info!(
                "Event subscription handler ended for session: {}",
                session_id_clone
            );
        });

        *self.stream_handle.lock().await = Some(stream_handle);

        info!("Event subscription started for session: {}", session_id);
        Ok(())
    }

    pub async fn send_message(
        &self,
        message: String,
        model: steer_core::config::model::ModelId,
    ) -> GrpcResult<()> {
        self.send_content_message(
            vec![crate::client_api::UserContent::Text { text: message }],
            model,
        )
        .await
    }

    pub async fn send_content_message(
        &self,
        content: Vec<crate::client_api::UserContent>,
        model: steer_core::config::model::ModelId,
    ) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let fallback_text = content
            .iter()
            .filter_map(|item| match item {
                crate::client_api::UserContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let proto_content: Vec<proto::UserContent> = content
            .into_iter()
            .map(|item| {
                let content = match item {
                    crate::client_api::UserContent::Text { text } => {
                        Some(proto::user_content::Content::Text(text))
                    }
                    crate::client_api::UserContent::CommandExecution {
                        command,
                        stdout,
                        stderr,
                        exit_code,
                    } => Some(proto::user_content::Content::CommandExecution(
                        proto::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        },
                    )),
                    crate::client_api::UserContent::Image { image } => {
                        let source = match image.source {
                            crate::client_api::ImageSource::SessionFile { relative_path } => {
                                Some(proto::image_content::Source::SessionFile(
                                    proto::SessionFileSource { relative_path },
                                ))
                            }
                            crate::client_api::ImageSource::DataUrl { data_url } => {
                                Some(proto::image_content::Source::DataUrl(
                                    proto::DataUrlSource { data_url },
                                ))
                            }
                            crate::client_api::ImageSource::Url { url } => {
                                Some(proto::image_content::Source::Url(proto::UrlSource { url }))
                            }
                        };

                        Some(proto::user_content::Content::Image(proto::ImageContent {
                            mime_type: image.mime_type,
                            source,
                            width: image.width,
                            height: image.height,
                            bytes: image.bytes,
                            sha256: image.sha256,
                        }))
                    }
                };
                proto::UserContent { content }
            })
            .collect();

        let steer_core::config::model::ModelId { provider, id } = model;
        let request = Request::new(proto::SendMessageRequest {
            session_id,
            message: fallback_text,
            content: proto_content,
            model: Some(proto::ModelSpec {
                provider_id: provider.storage_key(),
                model_id: id,
            }),
        });

        self.client
            .lock()
            .await
            .send_message(request)
            .await
            .map_err(Box::new)?;

        Ok(())
    }

    pub async fn edit_message(
        &self,
        message_id: String,
        content: Vec<crate::client_api::UserContent>,
        model: steer_core::config::model::ModelId,
    ) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let fallback_text = content
            .iter()
            .filter_map(|item| match item {
                crate::client_api::UserContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let proto_content: Vec<proto::UserContent> = content
            .into_iter()
            .map(|item| {
                let content = match item {
                    crate::client_api::UserContent::Text { text } => {
                        Some(proto::user_content::Content::Text(text))
                    }
                    crate::client_api::UserContent::CommandExecution {
                        command,
                        stdout,
                        stderr,
                        exit_code,
                    } => Some(proto::user_content::Content::CommandExecution(
                        proto::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        },
                    )),
                    crate::client_api::UserContent::Image { image } => {
                        let source = match image.source {
                            crate::client_api::ImageSource::SessionFile { relative_path } => {
                                Some(proto::image_content::Source::SessionFile(
                                    proto::SessionFileSource { relative_path },
                                ))
                            }
                            crate::client_api::ImageSource::DataUrl { data_url } => {
                                Some(proto::image_content::Source::DataUrl(
                                    proto::DataUrlSource { data_url },
                                ))
                            }
                            crate::client_api::ImageSource::Url { url } => {
                                Some(proto::image_content::Source::Url(proto::UrlSource { url }))
                            }
                        };

                        Some(proto::user_content::Content::Image(proto::ImageContent {
                            mime_type: image.mime_type,
                            source,
                            width: image.width,
                            height: image.height,
                            bytes: image.bytes,
                            sha256: image.sha256,
                        }))
                    }
                };
                proto::UserContent { content }
            })
            .collect();

        let steer_core::config::model::ModelId { provider, id } = model;
        let request = Request::new(proto::EditMessageRequest {
            session_id,
            message_id,
            new_content: fallback_text,
            content: proto_content,
            model: Some(proto::ModelSpec {
                provider_id: provider.storage_key(),
                model_id: id,
            }),
        });

        self.client
            .lock()
            .await
            .edit_message(request)
            .await
            .map_err(Box::new)?;

        Ok(())
    }

    pub async fn approve_tool(
        &self,
        tool_call_id: String,
        decision: crate::client_api::ApprovalDecision,
    ) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        use crate::client_api::ApprovalDecision;
        use proto::approval_decision::DecisionType;

        let decision_type = match decision {
            ApprovalDecision::Deny => DecisionType::Deny(true),
            ApprovalDecision::Once => DecisionType::Once(true),
            ApprovalDecision::AlwaysTool => DecisionType::AlwaysTool(true),
            ApprovalDecision::AlwaysBashPattern(pattern) => {
                DecisionType::AlwaysBashPattern(pattern)
            }
        };

        let request = Request::new(proto::ApproveToolRequest {
            session_id,
            tool_call_id,
            decision: Some(proto::ApprovalDecision {
                decision_type: Some(decision_type),
            }),
        });

        self.client
            .lock()
            .await
            .approve_tool(request)
            .await
            .map_err(Box::new)?;

        Ok(())
    }

    pub async fn switch_primary_agent(&self, primary_agent_id: String) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let request = Request::new(proto::SwitchPrimaryAgentRequest {
            session_id,
            primary_agent_id,
        });

        self.client
            .lock()
            .await
            .switch_primary_agent(request)
            .await
            .map_err(Box::new)?;

        Ok(())
    }

    pub async fn cancel_operation(&self) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let request = Request::new(proto::CancelOperationRequest { session_id });

        self.client
            .lock()
            .await
            .cancel_operation(request)
            .await
            .map_err(Box::new)?;

        Ok(())
    }

    pub async fn compact_session(
        &self,
        model: steer_core::config::model::ModelId,
    ) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let request = Request::new(proto::CompactSessionRequest {
            session_id,
            model: Some(model_to_proto(model)),
        });

        self.client
            .lock()
            .await
            .compact_session(request)
            .await
            .map_err(Box::new)?;

        Ok(())
    }

    pub async fn execute_bash_command(&self, command: String) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let request = Request::new(proto::ExecuteBashCommandRequest {
            session_id,
            command,
        });

        self.client
            .lock()
            .await
            .execute_bash_command(request)
            .await
            .map_err(Box::new)?;

        Ok(())
    }

    pub async fn dequeue_queued_item(&self) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let request = Request::new(proto::DequeueQueuedItemRequest { session_id });

        self.client
            .lock()
            .await
            .dequeue_queued_item(request)
            .await
            .map_err(Box::new)?;

        Ok(())
    }

    pub async fn subscribe_client_events(&self) -> GrpcResult<mpsc::Receiver<ClientEvent>> {
        let mut guard = self.client_event_rx.lock().await;
        if let Some(receiver) = guard.take() {
            Ok(receiver)
        } else {
            let reason = "Client events already subscribed".to_string();
            warn!("{reason}");
            Err(GrpcError::InvalidSessionState { reason })
        }
    }

    pub async fn session_id(&self) -> Option<String> {
        self.session_id.lock().await.clone()
    }

    pub async fn list_sessions(&self) -> GrpcResult<Vec<SessionInfo>> {
        debug!("Listing sessions from gRPC server");

        let request = Request::new(ListSessionsRequest {
            filter: None,
            page_size: None,
            page_token: None,
        });

        let response = self
            .client
            .lock()
            .await
            .list_sessions(request)
            .await
            .map_err(Box::new)?;
        let sessions_response = response.into_inner();

        Ok(sessions_response.sessions)
    }

    pub async fn get_session(&self, session_id: &str) -> GrpcResult<Option<SessionState>> {
        debug!("Getting session {} from gRPC server", session_id);

        let request = Request::new(GetSessionRequest {
            session_id: session_id.to_string(),
        });

        let mut stream = self
            .client
            .lock()
            .await
            .get_session(request)
            .await
            .map_err(GrpcError::from)?
            .into_inner();

        let mut header = None;
        let mut messages = Vec::new();
        let mut footer = None;

        while let Some(response) = stream.message().await.map_err(GrpcError::from)? {
            match response.chunk {
                Some(proto::get_session_response::Chunk::Header(h)) => header = Some(h),
                Some(proto::get_session_response::Chunk::Message(m)) => messages.push(m),
                Some(proto::get_session_response::Chunk::Footer(f)) => footer = Some(f),
                None => {}
            }
        }

        match (header, footer) {
            (Some(h), Some(f)) => Ok(Some(SessionState {
                id: h.id,
                created_at: h.created_at,
                updated_at: h.updated_at,
                config: h.config,
                messages,
                approved_tools: f.approved_tools,
                last_event_sequence: h.last_event_sequence,
                metadata: f.metadata,
            })),
            _ => Ok(None),
        }
    }

    pub async fn delete_session(&self, session_id: &str) -> GrpcResult<bool> {
        debug!("Deleting session {} from gRPC server", session_id);

        let request = Request::new(DeleteSessionRequest {
            session_id: session_id.to_string(),
        });

        match self.client.lock().await.delete_session(request).await {
            Ok(_) => {
                info!("Successfully deleted session: {}", session_id);
                Ok(true)
            }
            Err(status) if status.code() == tonic::Code::NotFound => Ok(false),
            Err(e) => Err(GrpcError::from(e)),
        }
    }

    pub async fn get_conversation(
        &self,
        session_id: &str,
    ) -> GrpcResult<(Vec<Message>, Vec<String>, Vec<String>)> {
        info!(
            "Client adapter getting conversation for session: {}",
            session_id
        );

        let mut stream = self
            .client
            .lock()
            .await
            .get_conversation(GetConversationRequest {
                session_id: session_id.to_string(),
            })
            .await
            .map_err(Box::new)?
            .into_inner();

        let mut messages = Vec::new();
        let mut approved_tools = Vec::new();
        let mut compaction_summary_ids = Vec::new();

        while let Some(response) = stream.message().await.map_err(GrpcError::from)? {
            match response.chunk {
                Some(proto::get_conversation_response::Chunk::Message(proto_msg)) => {
                    match proto_to_message(proto_msg) {
                        Ok(msg) => messages.push(msg),
                        Err(e) => {
                            warn!("Failed to convert message: {}", e);
                            return Err(GrpcError::ConversionError(e));
                        }
                    }
                }
                Some(proto::get_conversation_response::Chunk::Footer(footer)) => {
                    approved_tools = footer.approved_tools;
                    compaction_summary_ids = footer.compaction_summary_ids;
                }
                None => {}
            }
        }

        info!(
            "Successfully converted {} messages from GetConversation response",
            messages.len()
        );

        Ok((messages, approved_tools, compaction_summary_ids))
    }

    pub async fn shutdown(self) {
        if let Some(handle) = self.stream_handle.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }

        if let Some(session_id) = &*self.session_id.lock().await {
            info!("GrpcClientAdapter shut down for session: {}", session_id);
        }
    }

    pub async fn get_mcp_servers(&self) -> GrpcResult<Vec<McpServerInfo>> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let request = Request::new(GetMcpServersRequest {
            session_id: session_id.clone(),
        });

        let response = self
            .client
            .lock()
            .await
            .get_mcp_servers(request)
            .await
            .map_err(Box::new)?;

        let servers = response
            .into_inner()
            .servers
            .into_iter()
            .filter_map(|s| proto_to_mcp_server_info(s).ok())
            .collect();

        Ok(servers)
    }

    pub async fn resolve_model(
        &self,
        input: &str,
    ) -> GrpcResult<steer_core::config::model::ModelId> {
        let request = Request::new(proto::ResolveModelRequest {
            input: input.to_string(),
        });

        let response = self
            .client
            .lock()
            .await
            .resolve_model(request)
            .await
            .map_err(Box::new)?;

        let inner = response.into_inner();
        let model_spec = inner.model.ok_or_else(|| GrpcError::InvalidSessionState {
            reason: format!("Server returned no model for input '{input}'"),
        })?;

        let provider_id: steer_core::config::provider::ProviderId =
            serde_json::from_value(serde_json::Value::String(model_spec.provider_id.clone()))
                .map_err(|_| GrpcError::InvalidSessionState {
                    reason: format!(
                        "Invalid provider ID from server: {}",
                        model_spec.provider_id
                    ),
                })?;

        Ok(steer_core::config::model::ModelId::new(
            provider_id,
            model_spec.model_id,
        ))
    }

    pub async fn get_default_model(&self) -> GrpcResult<steer_core::config::model::ModelId> {
        let request = Request::new(GetDefaultModelRequest {});

        let response = self
            .client
            .lock()
            .await
            .get_default_model(request)
            .await
            .map_err(Box::new)?;

        let inner = response.into_inner();
        let model_spec = inner.model.ok_or_else(|| GrpcError::InvalidSessionState {
            reason: "Server returned no default model".to_string(),
        })?;

        let provider_id: steer_core::config::provider::ProviderId =
            serde_json::from_value(serde_json::Value::String(model_spec.provider_id.clone()))
                .map_err(|_| GrpcError::InvalidSessionState {
                    reason: format!(
                        "Invalid provider ID from server: {}",
                        model_spec.provider_id
                    ),
                })?;

        Ok(steer_core::config::model::ModelId::new(
            provider_id,
            model_spec.model_id,
        ))
    }

    pub async fn list_providers(&self) -> GrpcResult<Vec<ProviderInfo>> {
        let request = Request::new(proto::ListProvidersRequest {});
        let response = self
            .client
            .lock()
            .await
            .list_providers(request)
            .await
            .map_err(Box::new)?;
        let providers = response
            .into_inner()
            .providers
            .into_iter()
            .map(proto_to_provider_info)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(providers)
    }

    pub async fn list_primary_agents(&self) -> GrpcResult<Vec<PrimaryAgentSpec>> {
        let request = Request::new(proto::ListPrimaryAgentsRequest {});
        let response = self
            .client
            .lock()
            .await
            .list_primary_agents(request)
            .await
            .map_err(Box::new)?;

        let agents = response
            .into_inner()
            .agents
            .into_iter()
            .map(proto_to_primary_agent_spec)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(agents)
    }

    pub async fn get_provider_auth_status(
        &self,
        provider_id: Option<String>,
    ) -> GrpcResult<Vec<ProviderAuthStatus>> {
        let request = Request::new(proto::GetProviderAuthStatusRequest { provider_id });
        let response = self
            .client
            .lock()
            .await
            .get_provider_auth_status(request)
            .await
            .map_err(Box::new)?;
        let statuses = response
            .into_inner()
            .statuses
            .into_iter()
            .map(proto_to_provider_auth_status)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(statuses)
    }

    pub async fn start_auth(&self, provider_id: String) -> GrpcResult<StartAuthResponse> {
        let request = Request::new(proto::StartAuthRequest { provider_id });
        let response = self
            .client
            .lock()
            .await
            .start_auth(request)
            .await
            .map_err(Box::new)?;
        proto_to_start_auth_response(response.into_inner()).map_err(GrpcError::from)
    }

    pub async fn send_auth_input(
        &self,
        flow_id: String,
        input: String,
    ) -> GrpcResult<crate::client_api::AuthProgress> {
        let request = Request::new(proto::SendAuthInputRequest { flow_id, input });
        let response = self
            .client
            .lock()
            .await
            .send_auth_input(request)
            .await
            .map_err(Box::new)?;
        let progress =
            response
                .into_inner()
                .progress
                .ok_or_else(|| GrpcError::InvalidSessionState {
                    reason: "Missing auth progress in response".to_string(),
                })?;
        crate::grpc::conversions::proto_to_auth_progress(progress).map_err(GrpcError::from)
    }

    pub async fn get_auth_progress(
        &self,
        flow_id: String,
    ) -> GrpcResult<crate::client_api::AuthProgress> {
        let request = Request::new(proto::GetAuthProgressRequest { flow_id });
        let response = self
            .client
            .lock()
            .await
            .get_auth_progress(request)
            .await
            .map_err(Box::new)?;
        let progress =
            response
                .into_inner()
                .progress
                .ok_or_else(|| GrpcError::InvalidSessionState {
                    reason: "Missing auth progress in response".to_string(),
                })?;
        crate::grpc::conversions::proto_to_auth_progress(progress).map_err(GrpcError::from)
    }

    pub async fn cancel_auth(&self, flow_id: String) -> GrpcResult<()> {
        let request = Request::new(proto::CancelAuthRequest { flow_id });
        self.client
            .lock()
            .await
            .cancel_auth(request)
            .await
            .map_err(Box::new)?;
        Ok(())
    }

    pub async fn list_models(
        &self,
        provider_id: Option<String>,
    ) -> GrpcResult<Vec<proto::ProviderModel>> {
        let request = Request::new(proto::ListModelsRequest { provider_id });

        let response = self
            .client
            .lock()
            .await
            .list_models(request)
            .await
            .map_err(Box::new)?;

        Ok(response.into_inner().models)
    }

    pub async fn list_workspace_files(&self) -> GrpcResult<Vec<String>> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let request = Request::new(proto::ListFilesRequest {
            session_id,
            query: String::new(),
            max_results: 0,
        });

        let mut stream = self
            .client
            .lock()
            .await
            .list_files(request)
            .await
            .map_err(Box::new)?
            .into_inner();

        let mut all_files = Vec::new();
        while let Some(response) = stream.message().await.map_err(Box::new)? {
            all_files.extend(response.paths);
        }

        Ok(all_files)
    }

    pub async fn list_workspaces(
        &self,
        environment_id: Option<String>,
    ) -> GrpcResult<Vec<steer_workspace::WorkspaceInfo>> {
        let request = Request::new(ListWorkspacesRequest {
            environment_id: environment_id.unwrap_or_default(),
        });
        let response = self
            .client
            .lock()
            .await
            .list_workspaces(request)
            .await
            .map_err(Box::new)?;

        let workspaces = response
            .into_inner()
            .workspaces
            .into_iter()
            .map(proto_to_workspace_info)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(workspaces)
    }

    pub async fn list_repos(
        &self,
        environment_id: Option<String>,
    ) -> GrpcResult<Vec<steer_workspace::RepoInfo>> {
        let request = Request::new(ListReposRequest {
            environment_id: environment_id.unwrap_or_default(),
        });
        let response = self
            .client
            .lock()
            .await
            .list_repos(request)
            .await
            .map_err(Box::new)?;

        let repos = response
            .into_inner()
            .repos
            .into_iter()
            .map(proto_to_repo_info)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(repos)
    }

    pub async fn resolve_repo(
        &self,
        environment_id: Option<String>,
        path: String,
    ) -> GrpcResult<steer_workspace::RepoInfo> {
        let request = Request::new(ResolveRepoRequest {
            environment_id: environment_id.unwrap_or_default(),
            path,
        });
        let response = self
            .client
            .lock()
            .await
            .resolve_repo(request)
            .await
            .map_err(Box::new)?;

        let repo = response
            .into_inner()
            .repo
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "Repo missing from response".to_string(),
            })?;

        Ok(proto_to_repo_info(repo)?)
    }

    pub async fn get_workspace_status(
        &self,
        workspace_id: &str,
    ) -> GrpcResult<steer_workspace::WorkspaceStatus> {
        let request = Request::new(GetWorkspaceStatusRequest {
            workspace_id: workspace_id.to_string(),
        });

        let response = self
            .client
            .lock()
            .await
            .get_workspace_status(request)
            .await
            .map_err(Box::new)?;

        let status =
            response
                .into_inner()
                .status
                .ok_or_else(|| GrpcError::InvalidSessionState {
                    reason: "Workspace status missing from response".to_string(),
                })?;

        Ok(proto_to_workspace_status(status)?)
    }
}

#[cfg(test)]
mod tests {
    use crate::grpc::conversions::tool_approval_policy_to_proto;
    use steer_core::session::{ApprovalRules, ToolApprovalPolicy, UnapprovedBehavior};
    use steer_proto::agent::v1::UnapprovedBehavior as ProtoBehavior;

    #[test]
    fn test_convert_tool_approval_policy() {
        let policy = ToolApprovalPolicy::default();
        let proto_policy = tool_approval_policy_to_proto(&policy);
        assert_eq!(proto_policy.default_behavior, ProtoBehavior::Prompt as i32);
        assert!(proto_policy.preapproved.is_some());

        let mut tools = std::collections::HashSet::new();
        tools.insert("bash".to_string());
        let policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Deny,
            preapproved: ApprovalRules {
                tools,
                per_tool: std::collections::HashMap::new(),
            },
        };
        let proto_policy = tool_approval_policy_to_proto(&policy);
        assert_eq!(proto_policy.default_behavior, ProtoBehavior::Deny as i32);
        let preapproved = proto_policy.preapproved.unwrap();
        assert!(preapproved.tools.contains(&"bash".to_string()));

        let policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Allow,
            preapproved: ApprovalRules::default(),
        };
        let proto_policy = tool_approval_policy_to_proto(&policy);
        assert_eq!(proto_policy.default_behavior, ProtoBehavior::Allow as i32);
    }
}
