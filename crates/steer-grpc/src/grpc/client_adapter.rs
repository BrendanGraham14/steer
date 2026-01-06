use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use crate::client_api::{ClientCommand, ClientEvent};
use crate::grpc::conversions::{
    client_command_to_proto, proto_to_client_event, proto_to_mcp_server_info, proto_to_message,
    session_tool_config_to_proto, tool_approval_policy_to_proto, workspace_config_to_proto,
};
use crate::grpc::error::GrpcError;

type GrpcResult<T> = std::result::Result<T, GrpcError>;

use steer_core::app::conversation::Message;
use steer_core::session::{McpServerInfo, SessionConfig};
use steer_proto::agent::v1::{
    self as proto, CreateSessionRequest, DeleteSessionRequest, GetConversationRequest,
    GetMcpServersRequest, GetSessionRequest, ListSessionsRequest, SessionInfo, SessionState,
    StreamSessionRequest, SubscribeRequest, agent_service_client::AgentServiceClient,
    stream_session_request::Message as StreamSessionRequestType,
};

pub struct AgentClient {
    client: Mutex<AgentServiceClient<Channel>>,
    session_id: Mutex<Option<String>>,
    command_tx: Mutex<Option<mpsc::Sender<StreamSessionRequest>>>,
    client_event_rx: Mutex<Option<mpsc::Receiver<ClientEvent>>>,
    stream_handle: Mutex<Option<JoinHandle<()>>>,
}

impl AgentClient {
    /// Connect to a gRPC server
    pub async fn connect(addr: &str) -> GrpcResult<Self> {
        info!("Connecting to gRPC server at {}", addr);

        let client = AgentServiceClient::connect(addr.to_string()).await?;

        info!("Successfully connected to gRPC server");

        Ok(Self {
            client: Mutex::new(client),
            session_id: Mutex::new(None),
            command_tx: Mutex::new(None),
            stream_handle: Mutex::new(None),
            client_event_rx: Mutex::new(None),
        })
    }

    pub async fn from_channel(channel: Channel) -> GrpcResult<Self> {
        info!("Creating gRPC client from provided channel");

        let client = AgentServiceClient::new(channel);

        Ok(Self {
            client: Mutex::new(client),
            session_id: Mutex::new(None),
            command_tx: Mutex::new(None),
            stream_handle: Mutex::new(None),
            client_event_rx: Mutex::new(None),
        })
    }

    /// Convenience constructor: spin up a localhost gRPC server and return a ready client.
    pub async fn local(default_model: steer_core::config::model::ModelId) -> GrpcResult<Self> {
        use crate::local_server::setup_local_grpc;
        let (channel, _server_handle) = setup_local_grpc(default_model, None).await?;
        Self::from_channel(channel).await
    }

    /// Create a new session on the server
    pub async fn create_session(&self, config: SessionConfig) -> GrpcResult<String> {
        debug!("Creating new session with gRPC server");

        let tool_policy = tool_approval_policy_to_proto(&config.tool_config.approval_policy);
        let workspace_config = workspace_config_to_proto(&config.workspace);
        let tool_config = session_tool_config_to_proto(&config.tool_config);

        let request = Request::new(CreateSessionRequest {
            tool_policy: Some(tool_policy),
            metadata: config.metadata,
            tool_config: Some(tool_config),
            workspace_config: Some(workspace_config),
            system_prompt: config.system_prompt,
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

    /// Activate (load) an existing dormant session and get its state
    pub async fn activate_session(
        &self,
        session_id: String,
    ) -> GrpcResult<(Vec<Message>, Vec<String>)> {
        info!("Activating remote session: {}", session_id);

        let mut stream = self
            .client
            .lock()
            .await
            .activate_session(proto::ActivateSessionRequest {
                session_id: session_id.clone(),
            })
            .await
            .map_err(Box::new)?
            .into_inner();

        let mut messages = Vec::new();
        let mut approved_tools = Vec::new();

        while let Some(response) = stream
            .message()
            .await
            .map_err(|e| GrpcError::CallFailed(Box::new(e)))?
        {
            match response.chunk {
                Some(proto::activate_session_response::Chunk::Message(proto_msg)) => {
                    match proto_to_message(proto_msg) {
                        Ok(msg) => messages.push(msg),
                        Err(e) => return Err(GrpcError::ConversionError(e)),
                    }
                }
                Some(proto::activate_session_response::Chunk::Footer(footer)) => {
                    approved_tools = footer.approved_tools;
                }
                None => {}
            }
        }

        *self.session_id.lock().await = Some(session_id);
        Ok((messages, approved_tools))
    }

    /// Start bidirectional streaming with the server using the new ClientEvent API
    pub async fn start_client_streaming(&self) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No session ID - call create_session or activate_session first".to_string(),
            })?;

        debug!("Starting client event stream for session: {}", session_id);

        let (cmd_tx, cmd_rx) = mpsc::channel::<StreamSessionRequest>(32);
        let (evt_tx, evt_rx) = mpsc::channel::<ClientEvent>(100);

        let outbound_stream = ReceiverStream::new(cmd_rx);
        let request = Request::new(outbound_stream);

        let response = self
            .client
            .lock()
            .await
            .stream_session(request)
            .await
            .map_err(Box::new)?;
        let mut inbound_stream = response.into_inner();

        let subscribe_msg = StreamSessionRequest {
            session_id: session_id.clone(),
            message: Some(StreamSessionRequestType::Subscribe(SubscribeRequest {
                event_types: vec![],
                since_sequence: None,
            })),
        };

        cmd_tx
            .send(subscribe_msg)
            .await
            .map_err(|_| GrpcError::StreamError("Failed to send subscribe message".to_string()))?;

        let session_id_clone = session_id.clone();
        let stream_handle = tokio::spawn(async move {
            info!(
                "Started client event stream handler for session: {}",
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
                "Client event stream handler ended for session: {}",
                session_id_clone
            );
        });

        *self.command_tx.lock().await = Some(cmd_tx);
        *self.stream_handle.lock().await = Some(stream_handle);
        *self.client_event_rx.lock().await = Some(evt_rx);

        info!("Client streaming started for session: {}", session_id);
        Ok(())
    }

    pub async fn send(&self, command: ClientCommand) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No active session".to_string(),
            })?;

        let command_tx = self
            .command_tx
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "Streaming not started".to_string(),
            })?;

        if let Some(message) = client_command_to_proto(command, &session_id) {
            command_tx.send(message).await.map_err(|_| {
                GrpcError::StreamError("Failed to send command - stream may be closed".to_string())
            })?;
        }

        Ok(())
    }

    pub async fn subscribe_client_events(&self) -> mpsc::Receiver<ClientEvent> {
        self.client_event_rx
            .lock()
            .await
            .take()
            .expect("Client event receiver already taken - only supports single subscription")
    }

    /// Get the current session ID
    pub async fn session_id(&self) -> Option<String> {
        self.session_id.lock().await.clone()
    }

    /// List sessions on the remote server
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

    /// Get session details from the remote server
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
            .map_err(|e| GrpcError::CallFailed(Box::new(e)))?
            .into_inner();

        let mut header = None;
        let mut messages = Vec::new();
        let mut tool_calls = std::collections::HashMap::new();
        let mut footer = None;

        while let Some(response) = stream
            .message()
            .await
            .map_err(|e| GrpcError::CallFailed(Box::new(e)))?
        {
            match response.chunk {
                Some(proto::get_session_response::Chunk::Header(h)) => header = Some(h),
                Some(proto::get_session_response::Chunk::Message(m)) => messages.push(m),
                Some(proto::get_session_response::Chunk::ToolCall(tc)) => {
                    if let Some(value) = tc.value {
                        tool_calls.insert(tc.key, value);
                    }
                }
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
                tool_calls,
                approved_tools: f.approved_tools,
                last_event_sequence: f.last_event_sequence,
                metadata: f.metadata,
            })),
            _ => Ok(None),
        }
    }

    /// Delete a session on the remote server
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
            Err(e) => Err(GrpcError::CallFailed(Box::new(e))),
        }
    }

    /// Get the current conversation for a session
    pub async fn get_conversation(
        &self,
        session_id: &str,
    ) -> GrpcResult<(Vec<Message>, Vec<String>)> {
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

        while let Some(response) = stream
            .message()
            .await
            .map_err(|e| GrpcError::CallFailed(Box::new(e)))?
        {
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
                }
                None => {}
            }
        }

        info!(
            "Successfully converted {} messages from GetConversation response",
            messages.len()
        );

        Ok((messages, approved_tools))
    }

    /// Shutdown the adapter and clean up resources
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

    /// Resolve a model string (alias or provider/model) to a ModelId
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

        // Convert proto ModelSpec to core ModelId
        // Try to deserialize the provider string using serde (same as ModelRegistry does)
        let provider_id: steer_core::config::provider::ProviderId =
            serde_json::from_value(serde_json::Value::String(model_spec.provider_id.clone()))
                .map_err(|_| GrpcError::InvalidSessionState {
                    reason: format!(
                        "Invalid provider ID from server: {}",
                        model_spec.provider_id
                    ),
                })?;

        Ok((provider_id, model_spec.model_id))
    }

    /// List providers from server
    pub async fn list_providers(&self) -> GrpcResult<Vec<proto::ProviderInfo>> {
        let request = Request::new(proto::ListProvidersRequest {});
        let response = self
            .client
            .lock()
            .await
            .list_providers(request)
            .await
            .map_err(Box::new)?;
        Ok(response.into_inner().providers)
    }

    /// Get provider auth status from server
    pub async fn get_provider_auth_status(
        &self,
        provider_id: Option<String>,
    ) -> GrpcResult<Vec<proto::ProviderAuthStatus>> {
        let request = Request::new(proto::GetProviderAuthStatusRequest { provider_id });
        let response = self
            .client
            .lock()
            .await
            .get_provider_auth_status(request)
            .await
            .map_err(Box::new)?;
        Ok(response.into_inner().statuses)
    }

    /// List available models (only recommended ones)
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
}

#[cfg(test)]
mod tests {
    use crate::grpc::conversions::tool_approval_policy_to_proto;
    use steer_core::session::ToolApprovalPolicy;
    use steer_proto::agent::v1::tool_approval_policy::Policy;

    #[test]
    fn test_convert_tool_approval_policy() {
        let policy = ToolApprovalPolicy::AlwaysAsk;
        let proto_policy = tool_approval_policy_to_proto(&policy);
        assert!(matches!(proto_policy.policy, Some(Policy::AlwaysAsk(_))));

        let mut tools = std::collections::HashSet::new();
        tools.insert("bash".to_string());
        let policy = ToolApprovalPolicy::PreApproved { tools };
        let proto_policy = tool_approval_policy_to_proto(&policy);
        assert!(matches!(proto_policy.policy, Some(Policy::PreApproved(_))));
    }
}
