use crate::grpc::conversions::message_to_proto;
use crate::grpc::session_manager_ext::SessionManagerExt;
use std::sync::Arc;
use steer_core::session::manager::SessionManager;
use steer_proto::agent::v1::{self as proto, *};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};

pub struct AgentServiceImpl {
    session_manager: Arc<SessionManager>,
    llm_config_provider: steer_core::config::LlmConfigProvider,
}

impl AgentServiceImpl {
    pub fn new(
        session_manager: Arc<SessionManager>,
        llm_config_provider: steer_core::config::LlmConfigProvider,
    ) -> Self {
        Self {
            session_manager,
            llm_config_provider,
        }
    }
}

#[tonic::async_trait]
impl agent_service_server::AgentService for AgentServiceImpl {
    type StreamSessionStream = ReceiverStream<Result<StreamSessionResponse, Status>>;
    type ListFilesStream = ReceiverStream<Result<ListFilesResponse, Status>>;
    type GetSessionStream =
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<GetSessionResponse, Status>> + Send>>;
    type GetConversationStream = std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<GetConversationResponse, Status>> + Send>,
    >;
    type ActivateSessionStream = std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<ActivateSessionResponse, Status>> + Send>,
    >;

    async fn stream_session(
        &self,
        request: Request<Streaming<StreamSessionRequest>>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        let mut client_stream = request.into_inner();
        let (tx, rx) = mpsc::channel(100);

        // Clone session manager and llm_config_provider for the stream handler task
        let session_manager = self.session_manager.clone();
        let llm_config_provider = self.llm_config_provider.clone();

        let _stream_task: tokio::task::JoinHandle<()> = tokio::spawn(async move {
            // Handle the first message to establish the session connection
            let (session_id, mut event_rx) = if let Some(client_message_result) =
                client_stream.message().await.transpose()
            {
                match client_message_result {
                    Ok(client_message) => {
                        let session_id = client_message.session_id.clone();

                        // Try to take the event receiver for this session
                        let receiver = match session_manager
                            .take_event_receiver(&client_message.session_id)
                            .await
                        {
                            Ok(receiver) => {
                                // Session is already active - TUI will call GetConversation RPC to get history
                                debug!("Session {} is already active, TUI should call GetConversation to retrieve history", session_id);
                                receiver
                            },
                            Err(steer_core::error::Error::SessionManager(steer_core::session::manager::SessionManagerError::SessionNotActive { session_id })) => {
                                info!("Session {} not active, attempting to resume", session_id);

                                // Try to resume the session
                                match try_resume_session(&session_manager, &session_id, &llm_config_provider).await {
                                    Ok(()) => {
                                        // Session resumed, try to take receiver again
                                        match session_manager.take_event_receiver(&session_id).await {
                                            Ok(receiver) => receiver,
                                            Err(e) => {
                                                error!("Failed to get event receiver after resuming session {}: {}", session_id, e);
                                                let _ = tx
                                                    .send(Err(Status::internal(format!(
                                                        "Failed to establish stream after resuming session: {e}"
                                                    ))))
                                                    .await;
                                                return;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to resume session {}: {}", session_id, e);
                                        let _ = tx
                                            .send(Err(e))
                                            .await;
                                        return;
                                    }
                                }
                            }
                            Err(steer_core::error::Error::SessionManager(steer_core::session::manager::SessionManagerError::SessionAlreadyHasListener { session_id })) => {
                                error!("Session already has an active stream: {}", session_id);
                                let _ = tx
                                    .send(Err(Status::already_exists(format!(
                                        "Session {session_id} already has an active stream"
                                    ))))
                                    .await;
                                return;
                            }
                            Err(e) => {
                                error!("Error taking event receiver: {}", e);
                                let _ = tx
                                    .send(Err(Status::internal(format!(
                                        "Error establishing stream: {e}"
                                    ))))
                                    .await;
                                return;
                            }
                        };

                        // Process the first message
                        if let Err(e) =
                            handle_client_message(&session_manager, client_message).await
                        {
                            error!("Error handling first client message: {}", e);
                            let _ = tx
                                .send(Err(Status::internal(format!(
                                    "Error processing message: {e}"
                                ))))
                                .await;
                            return;
                        }

                        (session_id, receiver)
                    }
                    Err(e) => {
                        error!("Error receiving first client message: {}", e);
                        let _ = tx.send(Err(Status::internal("Stream error"))).await;
                        return;
                    }
                }
            } else {
                error!("No initial client message received");
                let _ = tx.send(Err(Status::internal("No initial message"))).await;
                return;
            };

            let mut event_sequence = 0u64;

            // Mark session as having an active subscriber
            if let Err(e) = session_manager
                .increment_subscriber_count(&session_id)
                .await
            {
                warn!(
                    "Failed to increment subscriber count for session {}: {}",
                    session_id, e
                );
            }

            // Spawn task to handle outgoing events (App -> Client)
            let tx_clone = tx.clone();
            let session_id_clone = session_id.clone();
            let event_task = tokio::spawn(async move {
                while let Some(app_event) = event_rx.recv().await {
                    event_sequence += 1;
                    let server_event = match crate::grpc::conversions::app_event_to_server_event(
                        app_event,
                        event_sequence,
                    ) {
                        Ok(event) => event,
                        Err(e) => {
                            warn!("Failed to convert app event to server event: {}", e);
                            continue;
                        }
                    };

                    if let Err(e) = tx_clone.send(Ok(server_event)).await {
                        warn!("Failed to send event to client: {}", e);
                        break;
                    }
                }
                debug!(
                    "Event forwarding task ended for session: {}",
                    session_id_clone
                );
            });

            // Handle incoming messages (Client -> App)
            while let Some(client_message_result) = client_stream.message().await.transpose() {
                match client_message_result {
                    Ok(client_message) => {
                        // Touch the session to update last activity
                        if let Err(e) = session_manager.touch_session(&session_id).await {
                            warn!("Failed to touch session {}: {}", session_id, e);
                        }

                        if let Err(e) =
                            handle_client_message(&session_manager, client_message).await
                        {
                            error!("Error handling client message: {}", e);
                            let _ = tx
                                .send(Err(Status::internal(format!(
                                    "Error processing message: {e}"
                                ))))
                                .await;
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Error receiving client message: {}", e);
                        let _ = tx.send(Err(Status::internal("Stream error"))).await;
                        break;
                    }
                }
            }

            // Clean up
            event_task.abort();

            // Decrement subscriber count
            if let Err(e) = session_manager
                .decrement_subscriber_count(&session_id)
                .await
            {
                warn!(
                    "Failed to decrement subscriber count for session {}: {}",
                    session_id, e
                );
            }

            info!("Client stream ended for session: {}", session_id);

            // Check if we should suspend the session (no more subscribers)
            if let Err(e) = session_manager
                .maybe_suspend_idle_session(&session_id)
                .await
            {
                warn!("Failed to check/suspend idle session {}: {}", session_id, e);
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let req = request.into_inner();

        let app_config = steer_core::app::AppConfig {
            llm_config_provider: self.llm_config_provider.clone(),
        };

        match self
            .session_manager
            .create_session_grpc(req, app_config)
            .await
        {
            Ok((_session_id, session_info)) => Ok(Response::new(CreateSessionResponse {
                session: Some(session_info),
            })),
            Err(e) => {
                error!("Failed to create session: {}", e);
                Err(e.into())
            }
        }
    }

    async fn list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let _req = request.into_inner();

        // Create filter - for now just list all sessions
        let filter = steer_core::session::SessionFilter::default();

        match self.session_manager.list_sessions(filter).await {
            Ok(sessions) => {
                let proto_sessions = sessions
                    .into_iter()
                    .map(|session| SessionInfo {
                        id: session.id,
                        created_at: Some(prost_types::Timestamp::from(
                            std::time::SystemTime::from(session.created_at),
                        )),
                        updated_at: Some(prost_types::Timestamp::from(
                            std::time::SystemTime::from(session.updated_at),
                        )),
                        status: proto::SessionStatus::Active as i32,
                        metadata: Some(proto::SessionMetadata {
                            labels: session.metadata,
                            annotations: std::collections::HashMap::new(),
                        }),
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
        let session_manager = self.session_manager.clone();

        let stream = async_stream::try_stream! {
            match session_manager.get_session_proto(&req.session_id).await {
                Ok(Some(session_state)) => {
                    // Send header
                    yield GetSessionResponse {
                        chunk: Some(get_session_response::Chunk::Header(SessionStateHeader {
                            id: session_state.id,
                            created_at: session_state.created_at,
                            updated_at: session_state.updated_at,
                            config: session_state.config,
                        })),
                    };

                    // Stream messages one by one
                    for message in session_state.messages {
                        yield GetSessionResponse {
                            chunk: Some(get_session_response::Chunk::Message(message)),
                        };
                    }

                    // Stream tool calls
                    for (key, value) in session_state.tool_calls {
                        yield GetSessionResponse {
                            chunk: Some(get_session_response::Chunk::ToolCall(ToolCallStateEntry {
                                key,
                                value: Some(value),
                            })),
                        };
                    }

                    // Send footer
                    yield GetSessionResponse {
                        chunk: Some(get_session_response::Chunk::Footer(SessionStateFooter {
                            approved_tools: session_state.approved_tools,
                            last_event_sequence: session_state.last_event_sequence,
                            metadata: session_state.metadata,
                        })),
                    };
                }
                Ok(None) => {
                    Err(Status::not_found(format!(
                        "Session not found: {}",
                        req.session_id
                    )))?;
                }
                Err(e) => {
                    error!("Failed to get session: {}", e);
                    Err(Status::internal(format!("Failed to get session: {e}")))?;
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }

    async fn delete_session(
        &self,
        request: Request<DeleteSessionRequest>,
    ) -> Result<Response<DeleteSessionResponse>, Status> {
        let req = request.into_inner();

        match self.session_manager.delete_session(&req.session_id).await {
            Ok(true) => Ok(Response::new(DeleteSessionResponse {})),
            Ok(false) => Err(Status::not_found(format!(
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
        let session_manager = self.session_manager.clone();

        info!("GetConversation called for session: {}", req.session_id);

        let stream = async_stream::try_stream! {
            match session_manager.get_session_state(&req.session_id).await {
                Ok(Some(session_state)) => {
                    info!(
                        "Found session state with {} messages and {} approved tools",
                        session_state.messages.len(),
                        session_state.approved_tools.len()
                    );

                    // Stream messages one by one
                    for msg in session_state.messages {
                        let proto_msg = message_to_proto(msg.clone())
                            .map_err(|e| Status::internal(format!("Failed to convert message: {e}")))?;
                        yield GetConversationResponse {
                            chunk: Some(get_conversation_response::Chunk::Message(proto_msg)),
                        };
                    }

                    // Send footer with approved tools
                    yield GetConversationResponse {
                        chunk: Some(get_conversation_response::Chunk::Footer(GetConversationFooter {
                            approved_tools: session_state.approved_tools.into_iter().collect(),
                        })),
                    };
                }
                Ok(None) => {
                    Err(Status::not_found(format!(
                        "Session not found: {}",
                        req.session_id
                    )))?;
                }
                Err(e) => {
                    error!("Failed to get session state: {}", e);
                    Err(Status::internal(format!("Failed to get session state: {e}")))?;
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }

    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<SendMessageResponse>, Status> {
        let req = request.into_inner();

        let app_command = steer_core::app::AppCommand::ProcessUserInput(req.message);

        match self
            .session_manager
            .send_command(&req.session_id, app_command)
            .await
        {
            Ok(()) => {
                // Generate operation ID for tracking
                let operation_id = format!("op_{}", uuid::Uuid::new_v4());
                Ok(Response::new(SendMessageResponse {
                    operation: Some(Operation {
                        id: operation_id,
                        session_id: req.session_id,
                        r#type: OperationType::SendMessage as i32,
                        status: OperationStatus::Running as i32,
                        created_at: Some(
                            prost_types::Timestamp::from(std::time::SystemTime::now()),
                        ),
                        completed_at: None,
                        metadata: std::collections::HashMap::new(),
                    }),
                }))
            }
            Err(e) => {
                error!("Failed to send message: {}", e);
                Err(Status::internal(format!("Failed to send message: {e}")))
            }
        }
    }

    async fn approve_tool(
        &self,
        request: Request<ApproveToolRequest>,
    ) -> Result<Response<ApproveToolResponse>, Status> {
        let req = request.into_inner();

        let approval = match req.decision {
            Some(decision) => match decision {
                proto::ApprovalDecision {
                    decision_type: Some(proto::approval_decision::DecisionType::Deny(true)),
                } => steer_core::app::command::ApprovalType::Denied,
                proto::ApprovalDecision {
                    decision_type: Some(proto::approval_decision::DecisionType::Once(true)),
                } => steer_core::app::command::ApprovalType::Once,
                proto::ApprovalDecision {
                    decision_type: Some(proto::approval_decision::DecisionType::AlwaysTool(true)),
                } => steer_core::app::command::ApprovalType::AlwaysTool,
                proto::ApprovalDecision {
                    decision_type:
                        Some(proto::approval_decision::DecisionType::AlwaysBashPattern(pattern)),
                } => steer_core::app::command::ApprovalType::AlwaysBashPattern(pattern),
                _ => {
                    return Err(Status::invalid_argument(
                        "Invalid approval decision enum value",
                    ));
                }
            },
            None => {
                return Err(Status::invalid_argument("Missing approval decision"));
            }
        };

        let app_command = steer_core::app::AppCommand::HandleToolResponse {
            id: req.tool_call_id,
            approval,
        };

        match self
            .session_manager
            .send_command(&req.session_id, app_command)
            .await
        {
            Ok(()) => Ok(Response::new(ApproveToolResponse {})),
            Err(e) => {
                error!("Failed to approve tool: {}", e);
                Err(Status::internal(format!("Failed to approve tool: {e}")))
            }
        }
    }

    async fn activate_session(
        &self,
        request: Request<ActivateSessionRequest>,
    ) -> Result<Response<Self::ActivateSessionStream>, Status> {
        let req = request.into_inner();
        let session_manager = self.session_manager.clone();
        let llm_config_provider = self.llm_config_provider.clone();

        info!("ActivateSession called for {}", req.session_id);

        let stream = async_stream::try_stream! {
            // Check if already active or activate it
            let state = if let Ok(Some(state)) = session_manager
                .get_session_state(&req.session_id)
                .await
            {
                state
            } else {
                // Not active, so activate it
                let app_config = steer_core::app::AppConfig {
                    llm_config_provider: llm_config_provider.clone(),
                };

                session_manager
                    .resume_session(&req.session_id, app_config)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to resume session: {e}")))?;

                // Fetch state now that it's active
                session_manager
                    .get_session_state(&req.session_id)
                    .await
                    .map_err(|e| Status::internal(format!("Failed to get session state: {e}")))?
                    .ok_or_else(|| Status::not_found(format!("Session not found: {}", req.session_id)))?
            };

            // Stream messages one by one
            for msg in state.messages {
                let proto_msg = message_to_proto(msg)
                    .map_err(|e| Status::internal(format!("Failed to convert message: {e}")))?;
                yield ActivateSessionResponse {
                    chunk: Some(activate_session_response::Chunk::Message(proto_msg)),
                };
            }

            // Send footer with approved tools
            yield ActivateSessionResponse {
                chunk: Some(activate_session_response::Chunk::Footer(ActivateSessionFooter {
                    approved_tools: state.approved_tools.into_iter().collect(),
                })),
            };
        };

        Ok(Response::new(Box::pin(stream)))
    }

    async fn cancel_operation(
        &self,
        request: Request<CancelOperationRequest>,
    ) -> Result<Response<CancelOperationResponse>, Status> {
        let req = request.into_inner();

        let app_command = steer_core::app::AppCommand::CancelProcessing;

        match self
            .session_manager
            .send_command(&req.session_id, app_command)
            .await
        {
            Ok(()) => Ok(Response::new(CancelOperationResponse {})),
            Err(e) => {
                error!("Failed to cancel operation: {}", e);
                Err(Status::internal(format!("Failed to cancel operation: {e}")))
            }
        }
    }

    async fn list_files(
        &self,
        request: Request<ListFilesRequest>,
    ) -> Result<Response<Self::ListFilesStream>, Status> {
        let req = request.into_inner();

        debug!("ListFiles called for session: {}", req.session_id);

        // Get the session's workspace
        let workspace = match self
            .session_manager
            .get_session_workspace(&req.session_id)
            .await
        {
            Ok(Some(workspace)) => workspace,
            Ok(None) => {
                return Err(Status::not_found(format!(
                    "Session not found: {}",
                    req.session_id
                )));
            }
            Err(e) => {
                error!("Failed to get session workspace: {}", e);
                return Err(Status::internal(format!(
                    "Failed to get session workspace: {e}"
                )));
            }
        };

        // Create the response stream
        let (tx, rx) = mpsc::channel(100);

        // Spawn task to stream the files
        let _list_task: tokio::task::JoinHandle<()> = tokio::spawn(async move {
            // Get the file list from the workspace
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
                    // Stream files in chunks of 1000
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
}

async fn try_resume_session(
    session_manager: &SessionManager,
    session_id: &str,
    llm_config_provider: &steer_core::config::LlmConfigProvider,
) -> Result<(), Status> {
    let app_config = steer_core::app::AppConfig {
        llm_config_provider: llm_config_provider.clone(),
    };

    // Attempt to resume the session
    match session_manager.resume_session(session_id, app_config).await {
        Ok(_command_tx) => {
            info!("Successfully resumed session: {}", session_id);
            // TUI will call GetCurrentConversation when it connects
            Ok(())
        }
        Err(steer_core::error::Error::SessionManager(
            steer_core::session::manager::SessionManagerError::CapacityExceeded { current, max },
        )) => {
            warn!(
                "Cannot resume session {}: server at capacity ({}/{})",
                session_id, current, max
            );
            Err(Status::resource_exhausted(format!(
                "Server at maximum capacity ({current}/{max}). Cannot resume session."
            )))
        }
        Err(e) => {
            error!("Failed to resume session {}: {}", session_id, e);
            Err(Status::internal(format!("Failed to resume session: {e}")))
        }
    }
}

async fn handle_client_message(
    session_manager: &SessionManager,
    client_message: StreamSessionRequest,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!(
        "Handling client message for session: {}",
        client_message.session_id
    );

    if let Some(message) = client_message.message {
        match message {
            stream_session_request::Message::SendMessage(send_msg) => {
                // Convert to AppCommand - just process user input since that's what exists
                let app_command = steer_core::app::AppCommand::ProcessUserInput(send_msg.message);

                session_manager
                    .send_command(&client_message.session_id, app_command)
                    .await
                    .map_err(|e| format!("Failed to send message: {e}"))?;
            }

            stream_session_request::Message::ToolApproval(approval) => {
                // Convert approval decision using existing HandleToolResponse
                let approval_type = match approval.decision {
                    Some(decision) => match decision.decision_type {
                        Some(proto::approval_decision::DecisionType::Deny(_)) => {
                            steer_core::app::command::ApprovalType::Denied
                        }
                        Some(proto::approval_decision::DecisionType::Once(_)) => {
                            steer_core::app::command::ApprovalType::Once
                        }
                        Some(proto::approval_decision::DecisionType::AlwaysTool(_)) => {
                            steer_core::app::command::ApprovalType::AlwaysTool
                        }
                        Some(proto::approval_decision::DecisionType::AlwaysBashPattern(
                            pattern,
                        )) => steer_core::app::command::ApprovalType::AlwaysBashPattern(pattern),
                        None => {
                            return Err(
                                "Invalid approval decision: no decision type specified".into()
                            );
                        }
                    },
                    None => {
                        return Err("Invalid approval decision: no decision provided".into());
                    }
                };

                let app_command = steer_core::app::AppCommand::HandleToolResponse {
                    id: approval.tool_call_id,
                    approval: approval_type,
                };

                session_manager
                    .send_command(&client_message.session_id, app_command)
                    .await
                    .map_err(|e| format!("Failed to approve tool: {e}"))?;
            }

            stream_session_request::Message::Cancel(_cancel) => {
                // Use existing CancelProcessing command
                let app_command = steer_core::app::AppCommand::CancelProcessing;

                session_manager
                    .send_command(&client_message.session_id, app_command)
                    .await
                    .map_err(|e| format!("Failed to cancel operation: {e}"))?;
            }

            stream_session_request::Message::Subscribe(_subscribe_request) => {
                debug!("Subscribe message received - stream already established");
                // No action needed - stream is already active
            }

            stream_session_request::Message::UpdateConfig(_update_config) => {
                // UpdateConfig no longer supports changing the LLM provider
                // Tool config updates are handled separately
                debug!("UpdateConfig received but provider changes are no longer supported");
            }

            stream_session_request::Message::ExecuteCommand(execute_command) => {
                use steer_core::app::conversation::AppCommandType;
                let app_cmd_type = match AppCommandType::parse(&execute_command.command) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        return Err(format!("Failed to parse command: {e}").into());
                    }
                };
                let app_command = steer_core::app::AppCommand::ExecuteCommand(app_cmd_type);
                session_manager
                    .send_command(&client_message.session_id, app_command)
                    .await
                    .map_err(|e| format!("Failed to execute command: {e}"))?;
            }

            stream_session_request::Message::ExecuteBashCommand(execute_bash_command) => {
                let app_command = steer_core::app::AppCommand::ExecuteBashCommand {
                    command: execute_bash_command.command,
                };
                session_manager
                    .send_command(&client_message.session_id, app_command)
                    .await
                    .map_err(|e| format!("Failed to execute bash command: {e}"))?;
            }

            stream_session_request::Message::EditMessage(edit_message) => {
                let app_command = steer_core::app::AppCommand::EditMessage {
                    message_id: edit_message.message_id,
                    new_content: edit_message.new_content,
                };
                session_manager
                    .send_command(&client_message.session_id, app_command)
                    .await
                    .map_err(|e| format!("Failed to edit message: {e}"))?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use steer_core::api::Model;

    use std::collections::HashMap;
    use steer_core::session::state::WorkspaceConfig;
    use steer_core::session::stores::sqlite::SqliteSessionStore;
    use steer_core::session::{SessionConfig, SessionManagerConfig, SessionToolConfig};
    use steer_proto::agent::v1::agent_service_client::AgentServiceClient;
    use steer_proto::agent::v1::{SendMessageRequest, SubscribeRequest};
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use tokio_stream::StreamExt;

    fn create_test_app_config() -> steer_core::app::AppConfig {
        steer_core::test_utils::test_app_config()
    }

    async fn create_test_session_manager() -> (Arc<SessionManager>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteSessionStore::new(&db_path).await.unwrap());

        let config = SessionManagerConfig {
            max_concurrent_sessions: 100,
            default_model: Model::ClaudeSonnet4_20250514,
            auto_persist: true,
        };
        let session_manager = Arc::new(SessionManager::new(store, config));

        (session_manager, temp_dir)
    }

    async fn create_test_server() -> (String, Arc<SessionManager>, TempDir) {
        let (session_manager, temp_dir) = create_test_session_manager().await;

        let auth_storage = Arc::new(steer_core::test_utils::InMemoryAuthStorage::new());
        let llm_config_provider = steer_core::config::LlmConfigProvider::new(auth_storage);
        let service = AgentServiceImpl::new(session_manager.clone(), llm_config_provider);

        // Start server on random port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let _server_task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(agent_service_server::AgentServiceServer::new(service))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let url = format!("http://{addr}");
        (url, session_manager, temp_dir)
    }

    #[tokio::test]
    async fn test_session_cleanup_on_disconnect() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        // Create a session
        let session_config = SessionConfig {
            workspace: WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            },
            tool_config: SessionToolConfig::default(),
            system_prompt: None,
            metadata: HashMap::new(),
        };

        let app_config = create_test_app_config();

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // Verify session is active
        assert!(session_manager.is_session_active(&session_id).await);

        // Simulate a client connection by incrementing subscriber count
        session_manager
            .increment_subscriber_count(&session_id)
            .await
            .unwrap();

        // Verify session is still active
        assert!(session_manager.is_session_active(&session_id).await);

        // Simulate client disconnect by decrementing subscriber count
        session_manager
            .decrement_subscriber_count(&session_id)
            .await
            .unwrap();

        // Check if session should be suspended
        session_manager
            .maybe_suspend_idle_session(&session_id)
            .await
            .unwrap();

        // Verify session was suspended (not active in memory)
        assert!(
            !session_manager.is_session_active(&session_id).await,
            "Session should be suspended after last client disconnects"
        );

        // Verify session still exists in storage
        let session_info = session_manager.get_session(&session_id).await.unwrap();
        assert!(
            session_info.is_some(),
            "Session should still exist in storage after suspension"
        );
    }

    #[tokio::test]
    async fn test_session_with_multiple_subscribers() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        // Create a session
        let session_config = SessionConfig {
            workspace: WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            },
            tool_config: SessionToolConfig::default(),
            system_prompt: None,
            metadata: HashMap::new(),
        };

        let app_config = create_test_app_config();

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // Simulate two clients connecting
        session_manager
            .increment_subscriber_count(&session_id)
            .await
            .unwrap();
        session_manager
            .increment_subscriber_count(&session_id)
            .await
            .unwrap();

        // First client disconnects
        session_manager
            .decrement_subscriber_count(&session_id)
            .await
            .unwrap();
        session_manager
            .maybe_suspend_idle_session(&session_id)
            .await
            .unwrap();

        // Session should still be active (one subscriber remaining)
        assert!(
            session_manager.is_session_active(&session_id).await,
            "Session should remain active with one subscriber"
        );

        // Second client disconnects
        session_manager
            .decrement_subscriber_count(&session_id)
            .await
            .unwrap();
        session_manager
            .maybe_suspend_idle_session(&session_id)
            .await
            .unwrap();

        // Now session should be suspended
        assert!(
            !session_manager.is_session_active(&session_id).await,
            "Session should be suspended after all clients disconnect"
        );
    }

    #[tokio::test]
    async fn test_grpc_client_connect_disconnect_cleanup() {
        let (server_url, session_manager, _temp_dir) = create_test_server().await;

        // Create a session first
        let session_config = SessionConfig {
            workspace: WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            },
            tool_config: SessionToolConfig::default(),
            system_prompt: None,
            metadata: HashMap::new(),
        };

        let app_config = create_test_app_config();

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // Verify session is active
        assert!(session_manager.is_session_active(&session_id).await);

        // Connect client
        let mut client = AgentServiceClient::connect(server_url.clone())
            .await
            .unwrap();

        // Start streaming with subscribe message
        let request_stream = tokio_stream::iter(vec![StreamSessionRequest {
            session_id: session_id.clone(),
            message: Some(stream_session_request::Message::Subscribe(
                SubscribeRequest {
                    event_types: vec![],
                    since_sequence: None,
                },
            )),
        }]);

        let response = client.stream_session(request_stream).await.unwrap();
        let _stream = response.into_inner();

        // Send a test message to verify session is working
        let (msg_tx, msg_rx) = mpsc::channel(10);
        msg_tx
            .send(StreamSessionRequest {
                session_id: session_id.clone(),
                message: Some(stream_session_request::Message::SendMessage(
                    SendMessageRequest {
                        session_id: session_id.clone(),
                        message: "Hello, test!".to_string(),
                        attachments: vec![],
                    },
                )),
            })
            .await
            .unwrap();

        // Create new request stream with the message channel
        let request_stream = tokio_stream::wrappers::ReceiverStream::new(msg_rx);
        let response = client.stream_session(request_stream).await.unwrap();
        let mut stream = response.into_inner();

        // Wait for some response to verify session is working
        let timeout =
            tokio::time::timeout(tokio::time::Duration::from_secs(5), stream.next()).await;

        assert!(timeout.is_ok(), "Should receive at least one event");

        // Drop the stream to simulate client disconnect
        drop(stream);
        drop(msg_tx);

        // Give the server time to process the disconnect
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Verify session was suspended (not active in memory)
        assert!(
            !session_manager.is_session_active(&session_id).await,
            "Session should be suspended after client disconnect"
        );

        // Verify session still exists in storage
        let session_info = session_manager.get_session(&session_id).await.unwrap();
        assert!(
            session_info.is_some(),
            "Session should still exist in storage"
        );
    }

    #[tokio::test]
    async fn test_grpc_basic_session_resume() {
        let (server_url, session_manager, _temp_dir) = create_test_server().await;

        // Create a session
        let session_config = SessionConfig {
            workspace: WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            },
            tool_config: SessionToolConfig::default(),
            system_prompt: None,
            metadata: HashMap::new(),
        };

        let app_config = create_test_app_config();

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // Suspend the session manually to simulate a disconnected state
        session_manager.suspend_session(&session_id).await.unwrap();
        assert!(
            !session_manager.is_session_active(&session_id).await,
            "Session should be suspended"
        );

        // Try to reconnect - this should auto-resume the session
        let mut client = AgentServiceClient::connect(server_url.clone())
            .await
            .unwrap();

        // Use a channel to keep the stream alive
        let (msg_tx, msg_rx) = mpsc::channel(10);

        // Send initial subscribe message
        msg_tx
            .send(StreamSessionRequest {
                session_id: session_id.clone(),
                message: Some(stream_session_request::Message::Subscribe(
                    SubscribeRequest {
                        event_types: vec![],
                        since_sequence: None,
                    },
                )),
            })
            .await
            .unwrap();

        let request_stream = tokio_stream::wrappers::ReceiverStream::new(msg_rx);
        let response = client.stream_session(request_stream).await;

        // The connection should succeed (auto-resume should work)
        assert!(
            response.is_ok(),
            "Should be able to connect to suspended session (auto-resume)"
        );

        let stream = response.unwrap().into_inner();

        // Give time for auto-resume to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Session should be active again after auto-resume
        assert!(
            session_manager.is_session_active(&session_id).await,
            "Session should be active after auto-resume"
        );

        // Keep the stream alive a bit longer to ensure it stays active
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert!(
            session_manager.is_session_active(&session_id).await,
            "Session should remain active while client is connected"
        );

        // Clean up - drop the stream to disconnect
        drop(stream);
        drop(msg_tx);

        // Give time for cleanup to run
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Now session should be suspended again after disconnect
        assert!(
            !session_manager.is_session_active(&session_id).await,
            "Session should be suspended after client disconnects"
        );
    }
}
