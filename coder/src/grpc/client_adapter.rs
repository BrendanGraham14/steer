use anyhow::{Result, anyhow};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use crate::app::{AppCommand, AppEvent};
use crate::grpc::proto::{
    ApprovalDecision, CancelOperationRequest, ClientMessage, CreateSessionRequest,
    DeleteSessionRequest, GetSessionRequest, ListSessionsRequest, SendMessageRequest, ServerEvent,
    SessionInfo, SessionState, SubscribeRequest, ToolApprovalResponse,
    agent_service_client::AgentServiceClient, client_message::Message as ClientMessageType,
};
use crate::session::{SessionConfig, ToolApprovalPolicy};

/// Adapter that bridges TUI's AppCommand/AppEvent interface with gRPC streaming
pub struct GrpcClientAdapter {
    client: AgentServiceClient<Channel>,
    session_id: Option<String>,
    command_tx: Option<mpsc::Sender<ClientMessage>>,
    stream_handle: Option<JoinHandle<()>>,
}

impl GrpcClientAdapter {
    /// Connect to a gRPC server
    pub async fn connect(addr: &str) -> Result<Self> {
        info!("Connecting to gRPC server at {}", addr);

        let client = AgentServiceClient::connect(addr.to_string())
            .await
            .map_err(|e| anyhow!("Failed to connect to gRPC server: {}", e))?;

        info!("Successfully connected to gRPC server");

        Ok(Self {
            client,
            session_id: None,
            command_tx: None,
            stream_handle: None,
        })
    }

    /// Create a new session on the server
    pub async fn create_session(&mut self, config: SessionConfig) -> Result<String> {
        debug!("Creating new session with gRPC server");

        let tool_policy = convert_tool_approval_policy(&config.tool_policy);

        let request = Request::new(CreateSessionRequest {
            tool_policy: Some(tool_policy),
            metadata: config.metadata,
            tool_config: None, // TODO: Add tool config conversion if needed
            workspace_config: Some(crate::grpc::proto::WorkspaceConfig {
                config: Some(crate::grpc::proto::workspace_config::Config::Local(
                    crate::grpc::proto::LocalWorkspaceConfig {},
                )),
            }),
        });

        let response = self.client.create_session(request).await?;
        let session_info = response.into_inner();

        self.session_id = Some(session_info.id.clone());

        info!("Created session: {}", session_info.id);
        Ok(session_info.id)
    }

    /// Resume an existing session
    pub async fn resume_session(&mut self, session_id: String) -> Result<()> {
        // For now, just set the session ID - the session should already exist on the server
        self.session_id = Some(session_id.clone());
        info!("Resuming session: {}", session_id);
        Ok(())
    }

    /// Start bidirectional streaming with the server
    pub async fn start_streaming(&mut self) -> Result<mpsc::Receiver<AppEvent>> {
        let session_id = self.session_id.as_ref().ok_or_else(|| {
            anyhow!("No session ID - call create_session or resume_session first")
        })?;

        debug!("Starting bidirectional stream for session: {}", session_id);

        // Create channels for command and event communication
        let (cmd_tx, cmd_rx) = mpsc::channel::<ClientMessage>(32);
        let (evt_tx, evt_rx) = mpsc::channel::<AppEvent>(100);

        // Create the bidirectional stream
        let outbound_stream = ReceiverStream::new(cmd_rx);
        let request = Request::new(outbound_stream);

        let response = self.client.stream_session(request).await?;
        let mut inbound_stream = response.into_inner();

        // Send initial subscribe message
        let subscribe_msg = ClientMessage {
            session_id: session_id.clone(),
            message: Some(ClientMessageType::Subscribe(SubscribeRequest {
                event_types: vec![], // Subscribe to all events
                since_sequence: None,
            })),
        };

        cmd_tx
            .send(subscribe_msg)
            .await
            .map_err(|_| anyhow!("Failed to send subscribe message"))?;

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

                        if let Some(app_event) = convert_server_event_to_app_event(server_event) {
                            if let Err(e) = evt_tx.send(app_event).await {
                                warn!("Failed to forward event to TUI: {}", e);
                                break;
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
        self.command_tx = Some(cmd_tx);
        self.stream_handle = Some(stream_handle);
        // Don't store evt_rx, return it directly

        info!(
            "Bidirectional streaming started for session: {}",
            session_id
        );
        Ok(evt_rx)
    }

    /// Send a command to the server
    pub async fn send_command(&self, command: AppCommand) -> Result<()> {
        let session_id = self
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow!("No active session"))?;

        let command_tx = self
            .command_tx
            .as_ref()
            .ok_or_else(|| anyhow!("Streaming not started - call start_streaming first"))?;

        let message = convert_app_command_to_client_message(command, session_id)?;

        if let Some(message) = message {
            command_tx
                .send(message)
                .await
                .map_err(|_| anyhow!("Failed to send command - stream may be closed"))?;
        }

        Ok(())
    }

    /// Get the current session ID
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// List sessions on the remote server
    pub async fn list_sessions(&mut self) -> Result<Vec<SessionInfo>> {
        debug!("Listing sessions from gRPC server");

        let request = Request::new(ListSessionsRequest {
            filter: None,
            page_size: None,
            page_token: None,
        });

        let response = self.client.list_sessions(request).await?;
        let sessions_response = response.into_inner();

        Ok(sessions_response.sessions)
    }

    /// Get session details from the remote server
    pub async fn get_session(&mut self, session_id: &str) -> Result<Option<SessionState>> {
        debug!("Getting session {} from gRPC server", session_id);

        let request = Request::new(GetSessionRequest {
            session_id: session_id.to_string(),
        });

        match self.client.get_session(request).await {
            Ok(response) => {
                let session_state = response.into_inner();
                Ok(Some(session_state))
            }
            Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
            Err(e) => Err(anyhow!("Failed to get session: {}", e)),
        }
    }

    /// Delete a session on the remote server
    pub async fn delete_session(&mut self, session_id: &str) -> Result<bool> {
        debug!("Deleting session {} from gRPC server", session_id);

        let request = Request::new(DeleteSessionRequest {
            session_id: session_id.to_string(),
        });

        match self.client.delete_session(request).await {
            Ok(_) => {
                info!("Successfully deleted session: {}", session_id);
                Ok(true)
            }
            Err(status) if status.code() == tonic::Code::NotFound => Ok(false),
            Err(e) => Err(anyhow!("Failed to delete session: {}", e)),
        }
    }

    /// Shutdown the adapter and clean up resources
    pub async fn shutdown(mut self) {
        if let Some(handle) = self.stream_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        if let Some(session_id) = &self.session_id {
            info!("GrpcClientAdapter shut down for session: {}", session_id);
        }
    }
}

/// Convert TUI AppCommand to gRPC ClientMessage
fn convert_app_command_to_client_message(
    command: AppCommand,
    session_id: &str,
) -> Result<Option<ClientMessage>> {
    let message = match command {
        AppCommand::ProcessUserInput(text) => {
            Some(ClientMessageType::SendMessage(SendMessageRequest {
                session_id: session_id.to_string(),
                message: text,
                attachments: vec![],
            }))
        }

        AppCommand::HandleToolResponse {
            id,
            approved,
            always,
        } => {
            let decision = if always {
                ApprovalDecision::AlwaysApprove
            } else if approved {
                ApprovalDecision::Approve
            } else {
                ApprovalDecision::Deny
            };

            Some(ClientMessageType::ToolApproval(ToolApprovalResponse {
                tool_call_id: id,
                decision: decision as i32,
            }))
        }

        AppCommand::CancelProcessing => {
            Some(ClientMessageType::Cancel(CancelOperationRequest {
                session_id: session_id.to_string(),
                operation_id: String::new(), // Server will cancel current operation
            }))
        }

        // These commands don't map to gRPC messages
        AppCommand::ExecuteCommand(_) => None,
        AppCommand::ExecuteBashCommand { .. } => None,
        AppCommand::Shutdown => None,
        AppCommand::RequestToolApprovalInternal { .. } => None,
        AppCommand::RestoreMessage(_) => None,
        AppCommand::PreApproveTools(_) => None,
        AppCommand::GetCurrentConversation => None,
    };

    Ok(message.map(|msg| ClientMessage {
        session_id: session_id.to_string(),
        message: Some(msg),
    }))
}

/// Convert gRPC ServerEvent to TUI AppEvent
fn convert_server_event_to_app_event(event: ServerEvent) -> Option<AppEvent> {
    use crate::grpc::events::server_event_to_app_event;

    // Use the existing conversion function from grpc/events.rs
    server_event_to_app_event(event)
}

/// Convert internal ToolApprovalPolicy to protobuf
fn convert_tool_approval_policy(
    policy: &ToolApprovalPolicy,
) -> crate::grpc::proto::ToolApprovalPolicy {
    use crate::grpc::proto::{
        AlwaysAskPolicy, MixedPolicy, PreApprovedPolicy,
        ToolApprovalPolicy as ProtoToolApprovalPolicy, tool_approval_policy::Policy,
    };

    let policy_variant = match policy {
        ToolApprovalPolicy::AlwaysAsk => Policy::AlwaysAsk(AlwaysAskPolicy {
            timeout_ms: None,
            default_decision: ApprovalDecision::Deny as i32,
        }),
        ToolApprovalPolicy::PreApproved(tools) => Policy::PreApproved(PreApprovedPolicy {
            tools: tools.iter().cloned().collect(),
        }),
        ToolApprovalPolicy::Mixed {
            pre_approved,
            ask_for_others,
        } => Policy::Mixed(MixedPolicy {
            pre_approved_tools: pre_approved.iter().cloned().collect(),
            ask_for_others: *ask_for_others,
            timeout_ms: None,
            default_decision: ApprovalDecision::Deny as i32,
        }),
    };

    ProtoToolApprovalPolicy {
        policy: Some(policy_variant),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc::proto::tool_approval_policy::Policy;

    #[test]
    fn test_convert_tool_approval_policy() {
        let policy = ToolApprovalPolicy::AlwaysAsk;
        let proto_policy = convert_tool_approval_policy(&policy);
        assert!(matches!(proto_policy.policy, Some(Policy::AlwaysAsk(_))));

        let mut tools = std::collections::HashSet::new();
        tools.insert("bash".to_string());
        let policy = ToolApprovalPolicy::PreApproved(tools);
        let proto_policy = convert_tool_approval_policy(&policy);
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
