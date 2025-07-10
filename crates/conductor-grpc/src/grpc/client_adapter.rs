use async_trait::async_trait;
use conductor_core::error::Result;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use crate::grpc::conversions::{
    convert_app_command_to_client_message, proto_to_message, server_event_to_app_event,
    session_tool_config_to_proto, tool_approval_policy_to_proto, workspace_config_to_proto,
};
use crate::grpc::error::GrpcError;

type GrpcResult<T> = std::result::Result<T, GrpcError>;

use conductor_core::app::conversation::Message;
use conductor_core::app::io::{AppCommandSink, AppEventSource};
use conductor_core::app::{AppCommand, AppEvent};
use conductor_core::session::SessionConfig;
use conductor_proto::agent::v1::{
    self as proto, CreateSessionRequest, DeleteSessionRequest, GetConversationRequest,
    GetSessionRequest, ListSessionsRequest, SessionInfo, SessionState, StreamSessionRequest,
    SubscribeRequest, agent_service_client::AgentServiceClient,
    stream_session_request::Message as StreamSessionRequestType,
};

/// Adapter that bridges TUI's AppCommand/AppEvent interface with gRPC streaming
pub struct GrpcClientAdapter {
    client: Mutex<AgentServiceClient<Channel>>,
    session_id: Mutex<Option<String>>,
    command_tx: Mutex<Option<mpsc::Sender<StreamSessionRequest>>>,
    event_rx: Mutex<Option<mpsc::Receiver<AppEvent>>>,
    stream_handle: Mutex<Option<JoinHandle<()>>>,
}

impl GrpcClientAdapter {
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
            event_rx: Mutex::new(None),
        })
    }

    /// Create client from an existing channel (for in-memory connections)
    pub async fn from_channel(channel: Channel) -> GrpcResult<Self> {
        info!("Creating gRPC client from provided channel");

        let client = AgentServiceClient::new(channel);

        Ok(Self {
            client: Mutex::new(client),
            session_id: Mutex::new(None),
            command_tx: Mutex::new(None),
            stream_handle: Mutex::new(None),
            event_rx: Mutex::new(None),
        })
    }

    /// Convenience constructor: spin up a localhost gRPC server and return a ready client.
    pub async fn local(default_model: conductor_core::api::Model) -> GrpcResult<Self> {
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

        let response = self
            .client
            .lock()
            .await
            .activate_session(proto::ActivateSessionRequest {
                session_id: session_id.clone(),
            })
            .await
            .map_err(Box::new)?
            .into_inner();

        // Convert proto messages -> app messages with explicit error handling
        let mut messages = Vec::new();
        for proto_msg in response.messages.into_iter() {
            match proto_to_message(proto_msg) {
                Ok(msg) => messages.push(msg),
                Err(e) => {
                    return Err(GrpcError::ConversionError(e));
                }
            }
        }

        *self.session_id.lock().await = Some(session_id);
        Ok((messages, response.approved_tools))
    }

    /// Start bidirectional streaming with the server
    pub async fn start_streaming(&self) -> GrpcResult<()> {
        let session_id = self
            .session_id
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| GrpcError::InvalidSessionState {
                reason: "No session ID - call create_session or activate_session first".to_string(),
            })?;

        debug!("Starting bidirectional stream for session: {}", session_id);

        // Create channels for command and event communication
        let (cmd_tx, cmd_rx) = mpsc::channel::<StreamSessionRequest>(32);
        let (evt_tx, evt_rx) = mpsc::channel::<AppEvent>(100);

        // Create the bidirectional stream
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

        // Send initial subscribe message
        let subscribe_msg = StreamSessionRequest {
            session_id: session_id.clone(),
            message: Some(StreamSessionRequestType::Subscribe(SubscribeRequest {
                event_types: vec![], // Subscribe to all events
                since_sequence: None,
            })),
        };

        cmd_tx
            .send(subscribe_msg)
            .await
            .map_err(|_| GrpcError::StreamError("Failed to send subscribe message".to_string()))?;

        // Spawn task to handle incoming server events
        let session_id_clone = session_id.clone();
        let stream_handle = tokio::spawn(async move {
            info!(
                "Started event stream handler for session: {}",
                session_id_clone
            );

            while let Some(result) = inbound_stream.message().await.transpose() {
                match result {
                    Ok(server_event) => {
                        debug!(
                            "Received server event: sequence {}",
                            server_event.sequence_num
                        );

                        match server_event_to_app_event(server_event) {
                            Ok(app_event) => {
                                if let Err(e) = evt_tx.send(app_event).await {
                                    warn!("Failed to forward event to TUI: {}", e);
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("Failed to convert server event: {}", e);
                                // Continue processing other events instead of breaking
                            }
                        }
                    }
                    Err(e) => {
                        error!("gRPC stream error: {}", e);
                        break;
                    }
                }
            }

            info!(
                "Event stream handler ended for session: {}",
                session_id_clone
            );
        });

        // Store the handles
        *self.command_tx.lock().await = Some(cmd_tx);
        *self.stream_handle.lock().await = Some(stream_handle);
        // store receiver
        *self.event_rx.lock().await = Some(evt_rx);

        info!(
            "Bidirectional streaming started for session: {}",
            session_id
        );
        Ok(())
    }

    /// Send a command to the server
    pub async fn send_command(&self, command: AppCommand) -> GrpcResult<()> {
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
                reason: "Streaming not started - call start_streaming first".to_string(),
            })?;

        let message = convert_app_command_to_client_message(command, &session_id)?;

        if let Some(message) = message {
            command_tx.send(message).await.map_err(|_| {
                GrpcError::StreamError("Failed to send command - stream may be closed".to_string())
            })?;
        }

        Ok(())
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

        match self.client.lock().await.get_session(request).await {
            Ok(response) => {
                let get_session_response = response.into_inner();
                Ok(get_session_response.session)
            }
            Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
            Err(e) => Err(GrpcError::CallFailed(Box::new(e))),
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

        let response = self
            .client
            .lock()
            .await
            .get_conversation(GetConversationRequest {
                session_id: session_id.to_string(),
            })
            .await
            .map_err(Box::new)?
            .into_inner();

        info!(
            "Received GetConversation response with {} messages and {} approved tools",
            response.messages.len(),
            response.approved_tools.len()
        );

        // Convert proto messages to app messages with explicit error handling
        let proto_message_count = response.messages.len();
        let mut messages = Vec::new();
        for proto_msg in response.messages.into_iter() {
            match proto_to_message(proto_msg) {
                Ok(msg) => messages.push(msg),
                Err(e) => {
                    return Err(GrpcError::ConversionError(e));
                }
            }
        }

        info!(
            "Converted {} proto messages to {} app messages",
            proto_message_count,
            messages.len()
        );

        Ok((messages, response.approved_tools))
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
}

#[async_trait]
impl AppCommandSink for GrpcClientAdapter {
    async fn send_command(&self, command: AppCommand) -> Result<()> {
        self.send_command(command)
            .await
            .map_err(|e| conductor_core::error::Error::InvalidOperation(e.to_string()))
    }
}

#[async_trait]
impl AppEventSource for GrpcClientAdapter {
    async fn subscribe(&self) -> mpsc::Receiver<AppEvent> {
        // This is a blocking operation in a trait that doesn't support async
        // We need to use block_on here
        self.event_rx.lock().await.take().expect(
            "Event receiver already taken - GrpcClientAdapter only supports single subscription",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc::conversions::tool_approval_policy_to_proto;
    use conductor_core::session::ToolApprovalPolicy;
    use conductor_proto::agent::v1::tool_approval_policy::Policy;

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

    #[test]
    fn test_convert_app_command_to_client_message() {
        let session_id = "test-session";

        let command = AppCommand::ProcessUserInput("Hello".to_string());
        let result = convert_app_command_to_client_message(command, session_id).unwrap();
        assert!(result.is_some());

        let command = AppCommand::Shutdown;
        let result = convert_app_command_to_client_message(command, session_id).unwrap();
        assert!(result.is_none());
    }
}
