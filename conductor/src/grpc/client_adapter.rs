use anyhow::{Result, anyhow};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use crate::app::{AppCommand, AppEvent};
use crate::app::conversation::Message;
use crate::grpc::conversions::{session_tool_config_to_proto, tool_approval_policy_to_proto, workspace_config_to_proto};
use crate::grpc::error::GrpcError;
use crate::grpc::proto::{
    ApprovalDecision, CancelOperationRequest, ClientMessage, CreateSessionRequest,
    DeleteSessionRequest, GetSessionRequest, GetConversationRequest, ListSessionsRequest, SendMessageRequest, ServerEvent,
    SessionInfo, SessionState, SubscribeRequest, ToolApprovalResponse, ActivateSessionRequest,
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

        let response = self.client.create_session(request).await?;
        let session_info = response.into_inner();

        self.session_id = Some(session_info.id.clone());

        info!("Created session: {}", session_info.id);
        Ok(session_info.id)
    }

    /// Activate (load) an existing dormant session and get its state
    pub async fn activate_session(&mut self, session_id: String) -> Result<(Vec<Message>, Vec<String>)> {
        info!("Activating remote session: {}", session_id);

        let response = self
            .client
            .activate_session(crate::grpc::proto::ActivateSessionRequest {
                session_id: session_id.clone(),
            })
            .await?
            .into_inner();

        // Convert proto messages -> app messages with explicit error handling
        let mut messages = Vec::new();
        for (i, proto_msg) in response.messages.into_iter().enumerate() {
            match proto_message_to_app_message(proto_msg) {
                Some(msg) => messages.push(msg),
                None => {
                    return Err(GrpcError::MessageConversionFailed {
                        index: i,
                        reason: "Failed to convert proto message to app message".to_string(),
                    }.into());
                }
            }
        }

        self.session_id = Some(session_id);
        Ok((messages, response.approved_tools))
    }

    /// Start bidirectional streaming with the server
    pub async fn start_streaming(&mut self) -> Result<mpsc::Receiver<AppEvent>> {
        let session_id = self.session_id.as_ref().ok_or_else(|| {
            anyhow!("No session ID - call create_session or activate_session first")
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

    /// Get the current conversation for a session
    pub async fn get_conversation(&mut self, session_id: &str) -> Result<(Vec<Message>, Vec<String>)> {
        info!("Client adapter getting conversation for session: {}", session_id);
        
        let response = self
            .client
            .get_conversation(GetConversationRequest {
                session_id: session_id.to_string(),
            })
            .await?
            .into_inner();

        info!("Received GetConversation response with {} messages and {} approved tools", 
            response.messages.len(), response.approved_tools.len());

        // Convert proto messages to app messages with explicit error handling
        let proto_message_count = response.messages.len();
        let mut messages = Vec::new();
        for (i, proto_msg) in response.messages.into_iter().enumerate() {
            match proto_message_to_app_message(proto_msg) {
                Some(msg) => messages.push(msg),
                None => {
                    return Err(GrpcError::MessageConversionFailed {
                        index: i,
                        reason: "Failed to convert proto message to app message".to_string(),
                    }.into());
                }
            }
        }
        
        info!("Converted {} proto messages to {} app messages", 
            proto_message_count, messages.len());

        Ok((messages, response.approved_tools))
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
        AppCommand::RestoreConversation { .. } => None,
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

/// Convert proto Message to app Message
fn proto_message_to_app_message(proto_msg: crate::grpc::proto::Message) -> Option<Message> {
    use crate::app::conversation::{AssistantContent, UserContent, ToolResult as ConversationToolResult};
    use crate::grpc::proto::{message, user_content, assistant_content, tool_result};
    use tools::ToolCall;
    
    let message_type = proto_msg.message.as_ref().map(|m| match m {
        message::Message::User(_) => "User",
        message::Message::Assistant(_) => "Assistant",
        message::Message::Tool(_) => "Tool",
    }).unwrap_or("Unknown");
    
    debug!("Converting proto message {} of type {}", proto_msg.id, message_type);
    
    match proto_msg.message? {
        message::Message::User(user_msg) => {
            let content = user_msg.content.into_iter().filter_map(|user_content| {
                match user_content.content? {
                    user_content::Content::Text(text) => {
                        Some(UserContent::Text { text })
                    }
                    user_content::Content::CommandExecution(cmd) => {
                        Some(UserContent::CommandExecution {
                            command: cmd.command,
                            stdout: cmd.stdout,
                            stderr: cmd.stderr,
                            exit_code: cmd.exit_code,
                        })
                    }
                }
            }).collect();
            Some(Message::User { 
                content, 
                timestamp: user_msg.timestamp, 
                id: proto_msg.id,
            })
        }
        message::Message::Assistant(assistant_msg) => {
            let content = assistant_msg.content.into_iter().filter_map(|assistant_content| {
                match assistant_content.content? {
                    assistant_content::Content::Text(text) => {
                        Some(AssistantContent::Text { text })
                    }
                    assistant_content::Content::ToolCall(tool_call) => {
                        let params = serde_json::from_str(&tool_call.parameters_json)
                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        
                        Some(AssistantContent::ToolCall { 
                            tool_call: ToolCall {
                                id: tool_call.id,
                                name: tool_call.name,
                                parameters: params,
                            }
                        })
                    }
                    assistant_content::Content::Thought(_) => {
                        // TODO: Handle thoughts properly when we implement them
                        None
                    }
                }
            }).collect();
            Some(Message::Assistant { 
                content, 
                timestamp: assistant_msg.timestamp, 
                id: proto_msg.id,
            })
        }
        message::Message::Tool(tool_msg) => {
            if let Some(result) = tool_msg.result {
                let tool_result = match result.result? {
                    tool_result::Result::Success(output) => {
                        ConversationToolResult::Success { output }
                    }
                    tool_result::Result::Error(error) => {
                        ConversationToolResult::Error { error }
                    }
                };
                Some(Message::Tool {
                    tool_use_id: tool_msg.tool_use_id,
                    result: tool_result,
                    timestamp: tool_msg.timestamp,
                    id: proto_msg.id,
                })
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc::conversions::tool_approval_policy_to_proto;
    use crate::grpc::proto::tool_approval_policy::Policy;

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
