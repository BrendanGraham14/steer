use crate::grpc::proto::*;
use crate::session::manager::SessionManager;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};

pub struct AgentServiceImpl {
    session_manager: Arc<SessionManager>,
}

impl AgentServiceImpl {
    pub fn new(session_manager: Arc<SessionManager>) -> Self {
        Self { session_manager }
    }
}

#[tonic::async_trait]
impl agent_service_server::AgentService for AgentServiceImpl {
    type StreamSessionStream = ReceiverStream<Result<ServerEvent, Status>>;

    async fn stream_session(
        &self,
        request: Request<Streaming<ClientMessage>>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        let mut client_stream = request.into_inner();
        let (tx, rx) = mpsc::channel(100);

        // Clone session manager for the stream handler task
        let session_manager = self.session_manager.clone();

        tokio::spawn(async move {
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
                                // Session is already active - send conversation history to new client
                                debug!("Session {} is already active, sending conversation history to new client", session_id);
                                if let Err(e) = session_manager.send_command(&session_id, crate::app::AppCommand::GetCurrentConversation).await {
                                    warn!("Failed to send GetCurrentConversation command for active session {}: {}", session_id, e);
                                } else {
                                    debug!("Sent GetCurrentConversation command for active session: {}", session_id);
                                }
                                receiver
                            },
                            Err(crate::session::manager::SessionManagerError::SessionNotActive { session_id }) => {
                                info!("Session {} not active, attempting to resume", session_id);

                                // Try to resume the session
                                match try_resume_session(&session_manager, &session_id).await {
                                    Ok(()) => {
                                        // Session resumed, try to take receiver again
                                        match session_manager.take_event_receiver(&session_id).await {
                                            Ok(receiver) => receiver,
                                            Err(e) => {
                                                error!("Failed to get event receiver after resuming session {}: {}", session_id, e);
                                                let _ = tx
                                                    .send(Err(Status::internal(format!(
                                                        "Failed to establish stream after resuming session: {}",
                                                        e
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
                            Err(crate::session::manager::SessionManagerError::SessionAlreadyHasListener { session_id }) => {
                                error!("Session already has an active stream: {}", session_id);
                                let _ = tx
                                    .send(Err(Status::already_exists(format!(
                                        "Session {} already has an active stream",
                                        session_id
                                    ))))
                                    .await;
                                return;
                            }
                            Err(e) => {
                                error!("Error taking event receiver: {}", e);
                                let _ = tx
                                    .send(Err(Status::internal(format!(
                                        "Error establishing stream: {}",
                                        e
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
                                    "Error processing message: {}",
                                    e
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
                    let server_event =
                        crate::grpc::events::app_event_to_server_event(app_event, event_sequence);

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
                                    "Error processing message: {}",
                                    e
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
    ) -> Result<Response<SessionInfo>, Status> {
        let req = request.into_inner();

        // Load LLM config from environment properly
        let llm_config = crate::config::LlmConfig::from_env()
            .map_err(|e| Status::internal(format!("Failed to load LLM config: {}", e)))?;

        let app_config = crate::app::AppConfig { llm_config };

        match self
            .session_manager
            .create_session_grpc(req, app_config)
            .await
        {
            Ok((_session_id, session_info)) => Ok(Response::new(session_info)),
            Err(e) => {
                error!("Failed to create session: {}", e);
                Err(Status::internal(format!("Failed to create session: {}", e)))
            }
        }
    }

    async fn list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let _req = request.into_inner();

        // Create filter - for now just list all sessions
        let filter = crate::session::SessionFilter::default();

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
                        status: crate::grpc::proto::SessionStatus::Active as i32,
                        metadata: Some(crate::grpc::proto::SessionMetadata {
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
                Err(Status::internal(format!("Failed to list sessions: {}", e)))
            }
        }
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<SessionState>, Status> {
        let req = request.into_inner();

        match self
            .session_manager
            .get_session_proto(&req.session_id)
            .await
        {
            Ok(Some(session_state)) => Ok(Response::new(session_state)),
            Ok(None) => Err(Status::not_found(format!(
                "Session not found: {}",
                req.session_id
            ))),
            Err(e) => {
                error!("Failed to get session: {}", e);
                Err(Status::internal(format!("Failed to get session: {}", e)))
            }
        }
    }

    async fn delete_session(
        &self,
        request: Request<DeleteSessionRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();

        match self.session_manager.delete_session(&req.session_id).await {
            Ok(true) => Ok(Response::new(())),
            Ok(false) => Err(Status::not_found(format!(
                "Session not found: {}",
                req.session_id
            ))),
            Err(e) => {
                error!("Failed to delete session: {}", e);
                Err(Status::internal(format!("Failed to delete session: {}", e)))
            }
        }
    }

    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<Operation>, Status> {
        let req = request.into_inner();

        let app_command = crate::app::AppCommand::ProcessUserInput(req.message);

        match self
            .session_manager
            .send_command(&req.session_id, app_command)
            .await
        {
            Ok(()) => {
                // Generate operation ID for tracking
                let operation_id = format!("op_{}", uuid::Uuid::new_v4());
                Ok(Response::new(Operation {
                    id: operation_id,
                    session_id: req.session_id,
                    r#type: OperationType::SendMessage as i32,
                    status: OperationStatus::Running as i32,
                    created_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                    completed_at: None,
                    metadata: std::collections::HashMap::new(),
                }))
            }
            Err(e) => {
                error!("Failed to send message: {}", e);
                Err(Status::internal(format!("Failed to send message: {}", e)))
            }
        }
    }

    async fn approve_tool(
        &self,
        request: Request<ApproveToolRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();

        let (approved, always) = match req.decision() {
            ApprovalDecision::Approve => (true, false),
            ApprovalDecision::AlwaysApprove => (true, true),
            ApprovalDecision::Deny => (false, false),
            ApprovalDecision::Unspecified => {
                return Err(Status::invalid_argument(
                    "Invalid approval decision: Unspecified",
                ));
            }
        };

        let app_command = crate::app::AppCommand::HandleToolResponse {
            id: req.tool_call_id,
            approved,
            always,
        };

        match self
            .session_manager
            .send_command(&req.session_id, app_command)
            .await
        {
            Ok(()) => Ok(Response::new(())),
            Err(e) => {
                error!("Failed to approve tool: {}", e);
                Err(Status::internal(format!("Failed to approve tool: {}", e)))
            }
        }
    }

    async fn cancel_operation(
        &self,
        request: Request<CancelOperationRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();

        let app_command = crate::app::AppCommand::CancelProcessing;

        match self
            .session_manager
            .send_command(&req.session_id, app_command)
            .await
        {
            Ok(()) => Ok(Response::new(())),
            Err(e) => {
                error!("Failed to cancel operation: {}", e);
                Err(Status::internal(format!(
                    "Failed to cancel operation: {}",
                    e
                )))
            }
        }
    }
}

async fn try_resume_session(
    session_manager: &SessionManager,
    session_id: &str,
) -> Result<(), Status> {
    // Load LLM config from environment
    let llm_config = crate::config::LlmConfig::from_env().map_err(|e| {
        Status::internal(format!(
            "Failed to load LLM config for session resume: {}",
            e
        ))
    })?;

    let app_config = crate::app::AppConfig { llm_config };

    // Attempt to resume the session
    match session_manager.resume_session(session_id, app_config).await {
        Ok((true, command_tx)) => {
            info!("Successfully resumed session: {}", session_id);

            // Send GetCurrentConversation command to trigger RestoredMessage events
            // This ensures the TUI gets populated with existing conversation history
            if let Err(e) = command_tx
                .send(crate::app::AppCommand::GetCurrentConversation)
                .await
            {
                warn!(
                    "Failed to send GetCurrentConversation command after resuming session {}: {}",
                    session_id, e
                );
            } else {
                debug!(
                    "Sent GetCurrentConversation command for resumed session: {}",
                    session_id
                );
            }

            Ok(())
        }
        Ok((false, _)) => {
            warn!("Session not found in storage: {}", session_id);
            Err(Status::not_found(format!(
                "Session not found: {}",
                session_id
            )))
        }
        Err(crate::session::manager::SessionManagerError::CapacityExceeded { current, max }) => {
            warn!(
                "Cannot resume session {}: server at capacity ({}/{})",
                session_id, current, max
            );
            Err(Status::resource_exhausted(format!(
                "Server at maximum capacity ({}/{}). Cannot resume session.",
                current, max
            )))
        }
        Err(e) => {
            error!("Failed to resume session {}: {}", session_id, e);
            Err(Status::internal(format!("Failed to resume session: {}", e)))
        }
    }
}

async fn handle_client_message(
    session_manager: &SessionManager,
    client_message: ClientMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!(
        "Handling client message for session: {}",
        client_message.session_id
    );

    if let Some(message) = client_message.message {
        match message {
            client_message::Message::SendMessage(send_msg) => {
                // Convert to AppCommand - just process user input since that's what exists
                let app_command = crate::app::AppCommand::ProcessUserInput(send_msg.message);

                session_manager
                    .send_command(&client_message.session_id, app_command)
                    .await
                    .map_err(|e| format!("Failed to send message: {}", e))?;
            }

            client_message::Message::ToolApproval(approval) => {
                // Convert approval decision using existing HandleToolResponse
                let (approved, always) = match approval.decision() {
                    ApprovalDecision::Approve => (true, false),
                    ApprovalDecision::AlwaysApprove => (true, true),
                    ApprovalDecision::Deny => (false, false),
                    ApprovalDecision::Unspecified => {
                        return Err("Invalid approval decision: Unspecified".into());
                    }
                };

                let app_command = crate::app::AppCommand::HandleToolResponse {
                    id: approval.tool_call_id,
                    approved,
                    always,
                };

                session_manager
                    .send_command(&client_message.session_id, app_command)
                    .await
                    .map_err(|e| format!("Failed to approve tool: {}", e))?;
            }

            client_message::Message::Cancel(_cancel) => {
                // Use existing CancelProcessing command
                let app_command = crate::app::AppCommand::CancelProcessing;

                session_manager
                    .send_command(&client_message.session_id, app_command)
                    .await
                    .map_err(|e| format!("Failed to cancel operation: {}", e))?;
            }

            client_message::Message::Subscribe(_subscribe_request) => {
                debug!("Subscribe message received - stream already established");
                // No action needed - stream is already active
            }

            client_message::Message::UpdateConfig(update_config) => {
                // TODO: Implement config updates
                debug!("Config update not yet implemented: {:?}", update_config);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Model;
    use crate::config::LlmConfig;
    use crate::events::StreamEventWithMetadata;
    use crate::grpc::proto::agent_service_client::AgentServiceClient;
    use crate::grpc::proto::{SendMessageRequest, SubscribeRequest};
    use crate::session::stores::sqlite::SqliteSessionStore;
    use crate::session::{
        SessionConfig, SessionManagerConfig, SessionToolConfig, ToolApprovalPolicy,
    };
    use std::collections::HashMap;
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use tokio_stream::StreamExt;

    async fn create_test_session_manager() -> (Arc<SessionManager>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteSessionStore::new(&db_path).await.unwrap());

        let (event_tx, _event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);
        let config = SessionManagerConfig {
            max_concurrent_sessions: 100,
            default_model: Model::ClaudeSonnet4_20250514,
            auto_persist: true,
        };
        let session_manager = Arc::new(SessionManager::new(store, config, event_tx));

        (session_manager, temp_dir)
    }

    async fn create_test_server() -> (String, Arc<SessionManager>, TempDir) {
        let (session_manager, temp_dir) = create_test_session_manager().await;

        let service = AgentServiceImpl::new(session_manager.clone());

        // Start server on random port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(agent_service_server::AgentServiceServer::new(service))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let url = format!("http://{}", addr);
        (url, session_manager, temp_dir)
    }

    #[tokio::test]
    async fn test_session_cleanup_on_disconnect() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        // Create a session
        let session_config = SessionConfig {
            tool_policy: ToolApprovalPolicy::AlwaysAsk,
            tool_config: SessionToolConfig::default(),
            metadata: HashMap::new(),
        };

        let app_config = crate::app::AppConfig {
            llm_config: LlmConfig {
                anthropic_api_key: None,
                openai_api_key: None,
                gemini_api_key: None,
            },
        };

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
            tool_policy: ToolApprovalPolicy::AlwaysAsk,
            tool_config: SessionToolConfig::default(),
            metadata: HashMap::new(),
        };

        let app_config = crate::app::AppConfig {
            llm_config: LlmConfig {
                anthropic_api_key: None,
                openai_api_key: None,
                gemini_api_key: None,
            },
        };

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
            tool_policy: ToolApprovalPolicy::AlwaysAsk,
            tool_config: SessionToolConfig::default(),
            metadata: HashMap::new(),
        };

        let app_config = crate::app::AppConfig {
            llm_config: LlmConfig {
                anthropic_api_key: None,
                openai_api_key: None,
                gemini_api_key: None,
            },
        };

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
        let request_stream = tokio_stream::iter(vec![ClientMessage {
            session_id: session_id.clone(),
            message: Some(client_message::Message::Subscribe(SubscribeRequest {
                event_types: vec![],
                since_sequence: None,
            })),
        }]);

        let response = client.stream_session(request_stream).await.unwrap();
        let stream = response.into_inner();

        // Send a test message to verify session is working
        let (msg_tx, msg_rx) = mpsc::channel(10);
        msg_tx
            .send(ClientMessage {
                session_id: session_id.clone(),
                message: Some(client_message::Message::SendMessage(SendMessageRequest {
                    session_id: session_id.clone(),
                    message: "Hello, test!".to_string(),
                })),
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
            tool_policy: ToolApprovalPolicy::AlwaysAsk,
            tool_config: SessionToolConfig::default(),
            metadata: HashMap::new(),
        };

        let app_config = crate::app::AppConfig {
            llm_config: LlmConfig {
                anthropic_api_key: None,
                openai_api_key: None,
                gemini_api_key: None,
            },
        };

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
            .send(ClientMessage {
                session_id: session_id.clone(),
                message: Some(client_message::Message::Subscribe(SubscribeRequest {
                    event_types: vec![],
                    since_sequence: None,
                })),
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
