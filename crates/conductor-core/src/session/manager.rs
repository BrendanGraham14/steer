use crate::api::Model;
use crate::app::{
    App, AppCommand, AppConfig, AppEvent, Conversation, Message as ConversationMessage,
};
use crate::error::{Error, Result};
use crate::events::{StreamEvent, StreamEventWithMetadata};
use crate::session::{
    Session, SessionConfig, SessionFilter, SessionInfo, SessionState, SessionStore,
    SessionStoreError, ToolCallUpdate,
};
use conductor_tools::ToolCall;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Session manager specific errors
#[derive(Debug, Error)]
pub enum SessionManagerError {
    #[error("Maximum session capacity reached ({current}/{max}). Cannot create new session.")]
    CapacityExceeded { current: usize, max: usize },

    #[error("Session not active: {session_id}")]
    SessionNotActive { session_id: String },

    #[error("Session {session_id} already has an active listener")]
    SessionAlreadyHasListener { session_id: String },

    #[error("Failed to create managed session: {message}")]
    CreationFailed { message: String },

    #[error("Storage error: {0}")]
    Storage(#[from] SessionStoreError),
}

/// Configuration for the SessionManager
#[derive(Debug, Clone)]
pub struct SessionManagerConfig {
    /// Maximum number of concurrent active sessions
    pub max_concurrent_sessions: usize,
    /// Default model for new sessions
    pub default_model: Model,
    /// Whether to automatically persist sessions
    pub auto_persist: bool,
}

/// A managed session contains both the session state and the App instance
pub struct ManagedSession {
    /// The session data
    pub session: Session,
    /// Command sender for the App
    pub command_tx: mpsc::Sender<AppCommand>,
    /// Event receiver from the App (for external consumers like TUI)
    pub event_rx: Option<mpsc::Receiver<AppEvent>>,
    /// Event sender for streaming events (deprecated, will be removed)
    pub event_tx: mpsc::Sender<StreamEvent>,
    /// Event subscriber count
    pub subscriber_count: usize,
    /// Last activity timestamp for cleanup
    pub last_activity: chrono::DateTime<chrono::Utc>,
    /// Handle to the app actor loop task
    pub app_task_handle: JoinHandle<()>,
    /// Handle to the event translation task
    pub event_task_handle: JoinHandle<()>,
}

impl ManagedSession {
    /// Create a new managed session
    pub async fn new(
        session: Session,
        app_config: AppConfig,
        event_tx: mpsc::Sender<StreamEvent>,
        store: Arc<dyn SessionStore>,
        global_event_tx: mpsc::Sender<StreamEventWithMetadata>,
        default_model: Model,
        conversation: Option<Conversation>,
    ) -> Result<Self> {
        // Create channels for the App
        let (app_event_tx, mut app_event_rx) = mpsc::channel(100);
        let (app_command_tx, app_command_rx) = mpsc::channel::<AppCommand>(32);

        // Always create external event channel
        let (external_event_tx, external_event_rx) = mpsc::channel(100);

        // Initialize the global command sender for tool approval requests
        crate::app::OpContext::init_command_tx(app_command_tx.clone());

        // Build backend registry from session tool config
        let backend_registry = session.config.build_registry().await?;

        // Build workspace from session config
        let workspace = session.build_workspace().await?;

        let tool_executor = Arc::new(crate::app::ToolExecutor::with_components(
            Some(workspace.clone()),
            Arc::new(backend_registry),
            Arc::new(crate::app::validation::ValidatorRegistry::new()),
        ));

        // Create the App instance with the configured tool executor and session config
        let mut app = if let Some(conv) = conversation {
            App::new_with_conversation(
                app_config,
                app_event_tx,
                default_model,
                workspace.clone(),
                tool_executor,
                Some(session.config.clone()),
                conv,
            )?
        } else {
            App::new(
                app_config,
                app_event_tx,
                default_model,
                workspace.clone(),
                tool_executor,
                Some(session.config.clone()),
            )?
        };

        // Set the initial model if specified in session metadata
        if let Some(model_str) = session.config.metadata.get("initial_model") {
            if let Ok(model) = model_str.parse::<crate::api::Model>() {
                let _ = app.set_model(model);
            }
        }

        // Spawn the app actor loop
        let app_task_handle = tokio::spawn(crate::app::app_actor_loop(app, app_command_rx));

        // Spawn the event translation/duplication task
        let session_id = session.id.clone();
        let store_clone = store.clone();
        let global_event_tx_clone = global_event_tx.clone();

        let event_task_handle = tokio::spawn(async move {
            while let Some(app_event) = app_event_rx.recv().await {
                // Always duplicate to external consumer
                if let Err(e) = external_event_tx.try_send(app_event.clone()) {
                    warn!(session_id = %session_id, "Failed to send event to external consumer: {}", e);
                }

                // Translate and persist
                if let Some(stream_event) = translate_app_event(app_event, &session_id) {
                    // Persist event
                    if let Ok(sequence_num) =
                        store_clone.append_event(&session_id, &stream_event).await
                    {
                        // Update session state in store
                        if let Err(e) =
                            update_session_state_for_event(&store_clone, &session_id, &stream_event)
                                .await
                        {
                            error!(session_id = %session_id, error = %e, "Failed to update session state");
                        }

                        // Broadcast
                        let event_with_metadata = StreamEventWithMetadata::new(
                            sequence_num,
                            session_id.clone(),
                            stream_event,
                        );
                        if let Err(e) = global_event_tx_clone.try_send(event_with_metadata) {
                            warn!(session_id = %session_id, error = %e, "Failed to broadcast event");
                        }
                    }
                }
            }
            info!(session_id = %session_id, "Event translation loop ended");
        });

        Ok(Self {
            session,
            command_tx: app_command_tx,
            event_rx: Some(external_event_rx),
            event_tx,
            subscriber_count: 0,
            last_activity: chrono::Utc::now(),
            app_task_handle,
            event_task_handle,
        })
    }

    /// Take the event receiver (can only be called once)
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<AppEvent>> {
        self.event_rx.take()
    }

    /// Update last activity timestamp
    pub fn touch(&mut self) {
        self.last_activity = chrono::Utc::now();
    }

    /// Check if session is inactive (no subscribers and old)
    pub fn is_inactive(&self, max_idle_time: chrono::Duration) -> bool {
        self.subscriber_count == 0 && chrono::Utc::now() - self.last_activity > max_idle_time
    }

    /// Shutdown the session gracefully
    pub async fn shutdown(self) {
        // Send shutdown command to app
        let _ = self.command_tx.send(AppCommand::Shutdown).await;

        // Wait for tasks to complete
        let _ = self.app_task_handle.await;
        let _ = self.event_task_handle.await;
    }
}

/// Manages multiple concurrent sessions
pub struct SessionManager {
    /// Active sessions with their App instances
    active_sessions: Arc<RwLock<HashMap<String, ManagedSession>>>,
    /// Session store for persistence
    store: Arc<dyn SessionStore>,
    /// Configuration
    config: SessionManagerConfig,
    /// Event broadcast channel
    event_tx: mpsc::Sender<StreamEventWithMetadata>,
}

impl SessionManager {
    /// Create a new SessionManager
    pub fn new(
        store: Arc<dyn SessionStore>,
        config: SessionManagerConfig,
        event_tx: mpsc::Sender<StreamEventWithMetadata>,
    ) -> Self {
        Self {
            active_sessions: Arc::new(RwLock::new(HashMap::new())),
            store,
            config,
            event_tx,
        }
    }

    /// Create a new session
    pub async fn create_session(
        &self,
        config: SessionConfig,
        app_config: AppConfig,
    ) -> Result<(String, mpsc::Sender<AppCommand>)> {
        let session_config = config;

        // Create session in store
        let session = self.store.create_session(session_config).await?;
        let session_id = session.id.clone();

        info!(session_id = %session_id, "Creating new session");

        // Check if we're at max capacity
        {
            let sessions = self.active_sessions.read().await;
            if sessions.len() >= self.config.max_concurrent_sessions {
                error!(
                    session_id = %session_id,
                    active_count = sessions.len(),
                    max_capacity = self.config.max_concurrent_sessions,
                    "Session creation rejected: at maximum capacity"
                );
                return Err(SessionManagerError::CapacityExceeded {
                    current: sessions.len(),
                    max: self.config.max_concurrent_sessions,
                }
                .into());
            }
        }

        // Create event channel for this session
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Create managed session with event translation
        let managed_session = ManagedSession::new(
            session.clone(),
            app_config,
            event_tx,
            self.store.clone(),
            self.event_tx.clone(),
            self.config.default_model,
            None,
        )
        .await
        .map_err(|e| SessionManagerError::CreationFailed {
            message: format!("Failed to create managed session: {e}"),
        })?;

        // Get command sender before moving into sessions map
        let command_tx = managed_session.command_tx.clone();

        // Add to active sessions
        {
            let mut sessions = self.active_sessions.write().await;
            sessions.insert(session_id.clone(), managed_session);
        }

        // Emit session created event
        let metadata = crate::events::SessionMetadata::from(&SessionInfo::from(&session));
        let event = StreamEvent::SessionCreated {
            session_id: session_id.clone(),
            metadata,
        };
        self.emit_event(session_id.clone(), event).await;

        info!(session_id = %session_id, "Session created and activated");
        Ok((session_id, command_tx))
    }

    /// Take the event receiver for a session (can only be called once per session)
    pub async fn take_event_receiver(&self, session_id: &str) -> Result<mpsc::Receiver<AppEvent>> {
        let mut sessions = self.active_sessions.write().await;
        match sessions.get_mut(session_id) {
            Some(managed_session) => {
                if let Some(receiver) = managed_session.take_event_rx() {
                    Ok(receiver)
                } else {
                    Err(SessionManagerError::SessionAlreadyHasListener {
                        session_id: session_id.to_string(),
                    }
                    .into())
                }
            }
            None => Err(SessionManagerError::SessionNotActive {
                session_id: session_id.to_string(),
            }
            .into()),
        }
    }

    /// Get session information
    pub async fn get_session(&self, session_id: &str) -> Result<Option<SessionInfo>> {
        // First check if it's active
        {
            let sessions = self.active_sessions.read().await;
            if let Some(managed_session) = sessions.get(session_id) {
                return Ok(Some(SessionInfo::from(&managed_session.session)));
            }
        }

        // If not active, load from store
        if let Some(session) = self.store.get_session(session_id).await? {
            Ok(Some(SessionInfo::from(&session)))
        } else {
            Ok(None)
        }
    }

    /// Get the workspace for a session
    pub async fn get_session_workspace(
        &self,
        session_id: &str,
    ) -> Result<Option<Arc<dyn crate::workspace::Workspace>>> {
        // First check if session is active
        {
            let active_sessions = self.active_sessions.read().await;
            if let Some(managed_session) = active_sessions.get(session_id) {
                // Session is active, rebuild workspace from config
                return Ok(Some(
                    managed_session
                        .session
                        .build_workspace()
                        .await
                        .map_err(|e| SessionManagerError::CreationFailed {
                            message: format!("Failed to build workspace: {e}"),
                        })?,
                ));
            }
        }

        // Session not active, try to load from store
        if let Some(session_info) = self.store.get_session(session_id).await? {
            let session = session_info;
            Ok(Some(session.build_workspace().await.map_err(|e| {
                SessionManagerError::CreationFailed {
                    message: format!("Failed to build workspace: {e}"),
                }
            })?))
        } else {
            Ok(None)
        }
    }

    /// Resume a session (load from storage and activate)
    pub async fn resume_session(
        &self,
        session_id: &str,
        app_config: AppConfig,
    ) -> Result<mpsc::Sender<AppCommand>> {
        // Check if already active
        {
            let sessions = self.active_sessions.read().await;
            if let Some(managed_session) = sessions.get(session_id) {
                debug!(session_id = %session_id, "Session already active");
                return Ok(managed_session.command_tx.clone());
            }
        }

        // Load from store
        let session = match self
            .store
            .get_session(session_id)
            .await
            .map_err(SessionManagerError::Storage)?
        {
            Some(session) => session,
            None => {
                debug!(session_id = %session_id, "Session not found in store");
                return Err(SessionManagerError::SessionNotActive {
                    session_id: session_id.to_string(),
                }
                .into());
            }
        };

        info!(session_id = %session_id, "Resuming session from storage");

        // Check capacity
        {
            let sessions = self.active_sessions.read().await;
            if sessions.len() >= self.config.max_concurrent_sessions {
                warn!(
                    session_id = %session_id,
                    active_count = sessions.len(),
                    max_capacity = self.config.max_concurrent_sessions,
                    "At maximum session capacity for resume"
                );
                // TODO: Implement eviction strategy
            }
        }

        // Create event channel for this session
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Create a conversation from the session state
        let conversation = Conversation {
            messages: session.state.messages.clone(),
            working_directory: session
                .config
                .workspace
                .get_path()
                .unwrap_or_default()
                .into(),
            current_thread_id: session
                .state
                .messages
                .last()
                .map(|m| *m.thread_id())
                .unwrap_or_else(Conversation::generate_thread_id),
        };

        // Create managed session with event translation
        let managed_session = ManagedSession::new(
            session.clone(),
            app_config,
            event_tx,
            self.store.clone(),
            self.event_tx.clone(),
            self.config.default_model,
            Some(conversation),
        )
        .await
        .map_err(|e| SessionManagerError::CreationFailed {
            message: format!("Failed to create managed session: {e}"),
        })?;

        // Get command sender before restoration
        let command_tx = managed_session.command_tx.clone();

        // Restore conversation history and approved tools atomically
        if !session.state.messages.is_empty() || !session.state.approved_tools.is_empty() {
            info!(
                session_id = %session_id,
                message_count = session.state.messages.len(),
                tool_count = session.state.approved_tools.len(),
                "Restoring conversation state"
            );

            command_tx
                .send(AppCommand::RestoreConversation {
                    messages: session.state.messages.clone(),
                    approved_tools: session.state.approved_tools.clone(),
                })
                .await
                .map_err(|_| SessionManagerError::CreationFailed {
                    message: "Failed to send restore command to App".to_string(),
                })?;
        }

        // Add to active sessions
        {
            let mut sessions = self.active_sessions.write().await;
            sessions.insert(session_id.to_string(), managed_session);
        }

        // Get the last event sequence for resume
        let last_sequence = session.state.last_event_sequence;

        // Emit session resumed event
        let event = StreamEvent::SessionResumed {
            session_id: session_id.to_string(),
            event_offset: last_sequence,
        };
        self.emit_event(session_id.to_string(), event).await;

        info!(session_id = %session_id, last_sequence = last_sequence, "Session resumed");
        Ok(command_tx)
    }

    /// Suspend a session (save to storage and deactivate)
    pub async fn suspend_session(&self, session_id: &str) -> Result<bool> {
        let managed_session = {
            let mut sessions = self.active_sessions.write().await;
            sessions.remove(session_id)
        };

        let managed_session = match managed_session {
            Some(session) => session,
            None => {
                debug!(session_id = %session_id, "Session not active, cannot suspend");
                return Ok(false);
            }
        };

        info!(session_id = %session_id, "Suspending session");

        // Save current state to store
        self.store.update_session(&managed_session.session).await?;

        // Emit session saved event
        let event = StreamEvent::SessionSaved {
            session_id: session_id.to_string(),
        };
        self.emit_event(session_id.to_string(), event).await;

        info!(session_id = %session_id, "Session suspended and saved");
        Ok(true)
    }

    /// Delete a session permanently
    pub async fn delete_session(&self, session_id: &str) -> Result<bool> {
        // Remove from active sessions first
        {
            let mut sessions = self.active_sessions.write().await;
            sessions.remove(session_id);
        }

        // Delete from store
        self.store.delete_session(session_id).await?;

        info!(session_id = %session_id, "Session deleted");
        Ok(true)
    }

    /// List sessions with filtering
    pub async fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionInfo>> {
        Ok(self.store.list_sessions(filter).await?)
    }

    /// Get active session IDs
    pub async fn get_active_sessions(&self) -> Vec<String> {
        let sessions = self.active_sessions.read().await;
        sessions.keys().cloned().collect()
    }

    /// Check if a session is active
    pub async fn is_session_active(&self, session_id: &str) -> bool {
        let sessions = self.active_sessions.read().await;
        sessions.contains_key(session_id)
    }

    /// Send a command to an active session
    pub async fn send_command(&self, session_id: &str, command: AppCommand) -> Result<()> {
        let sessions = self.active_sessions.read().await;
        if let Some(managed_session) = sessions.get(session_id) {
            managed_session.command_tx.send(command).await.map_err(|_| {
                Error::SessionManager(SessionManagerError::SessionNotActive {
                    session_id: session_id.to_string(),
                })
            })
        } else {
            Err(Error::SessionManager(
                SessionManagerError::SessionNotActive {
                    session_id: session_id.to_string(),
                },
            ))
        }
    }

    /// Update session state and persist if auto-persist is enabled
    pub async fn update_session_state(
        &self,
        session_id: &str,
        update_fn: impl FnOnce(&mut SessionState),
    ) -> Result<()> {
        {
            let mut sessions = self.active_sessions.write().await;
            if let Some(managed_session) = sessions.get_mut(session_id) {
                managed_session.touch();
                update_fn(&mut managed_session.session.state);
                managed_session.session.update_timestamp();

                // Auto-persist if enabled
                if self.config.auto_persist {
                    self.store.update_session(&managed_session.session).await?;
                }
            } else {
                return Err(SessionManagerError::SessionNotActive {
                    session_id: session_id.to_string(),
                }
                .into());
            }
        }

        Ok(())
    }

    /// Emit an event for a session
    pub async fn emit_event(&self, session_id: String, event: StreamEvent) {
        // Get next sequence number and store event
        let sequence_num = match self.store.append_event(&session_id, &event).await {
            Ok(seq) => seq,
            Err(e) => {
                error!(session_id = %session_id, error = %e, "Failed to persist event");
                return;
            }
        };

        // Update session state with new sequence number
        if let Err(e) = self
            .update_session_state(&session_id, |state| {
                state.last_event_sequence = sequence_num;
            })
            .await
        {
            error!(session_id = %session_id, error = %e, "Failed to update session sequence number");
        }

        // Create event with metadata
        let event_with_metadata =
            StreamEventWithMetadata::new(sequence_num, session_id.clone(), event);

        // Broadcast to subscribers
        if let Err(e) = self.event_tx.try_send(event_with_metadata) {
            warn!(error = %e, "Failed to broadcast event");
        }
    }

    /// Cleanup inactive sessions
    pub async fn cleanup_inactive_sessions(&self, max_idle_time: chrono::Duration) -> usize {
        let mut to_suspend = Vec::new();

        {
            let sessions = self.active_sessions.read().await;
            for (session_id, managed_session) in sessions.iter() {
                if managed_session.is_inactive(max_idle_time) {
                    to_suspend.push(session_id.clone());
                }
            }
        }

        let mut suspended_count = 0;
        for session_id in to_suspend {
            if let Ok(true) = self.suspend_session(&session_id).await {
                suspended_count += 1;
            }
        }

        if suspended_count > 0 {
            info!(
                suspended_count = suspended_count,
                "Cleaned up inactive sessions"
            );
        }

        suspended_count
    }

    /// Get session store reference
    pub fn store(&self) -> &Arc<dyn SessionStore> {
        &self.store
    }

    /// Increment the subscriber count for a session
    pub async fn increment_subscriber_count(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.active_sessions.write().await;
        if let Some(managed_session) = sessions.get_mut(session_id) {
            managed_session.subscriber_count += 1;
            managed_session.touch();
            debug!(
                session_id = %session_id,
                subscriber_count = managed_session.subscriber_count,
                "Incremented subscriber count"
            );
            Ok(())
        } else {
            Err(SessionManagerError::SessionNotActive {
                session_id: session_id.to_string(),
            }
            .into())
        }
    }

    /// Decrement the subscriber count for a session
    pub async fn decrement_subscriber_count(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.active_sessions.write().await;
        if let Some(managed_session) = sessions.get_mut(session_id) {
            managed_session.subscriber_count = managed_session.subscriber_count.saturating_sub(1);
            managed_session.touch();
            debug!(
                session_id = %session_id,
                subscriber_count = managed_session.subscriber_count,
                "Decremented subscriber count"
            );
            Ok(())
        } else {
            // Session might have already been cleaned up
            debug!(session_id = %session_id, "Session not active when decrementing subscriber count");
            Ok(())
        }
    }

    /// Touch a session to update its last activity timestamp
    pub async fn touch_session(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.active_sessions.write().await;
        if let Some(managed_session) = sessions.get_mut(session_id) {
            managed_session.touch();
            Ok(())
        } else {
            // Session might have been suspended, which is okay
            Ok(())
        }
    }

    /// Check if a session should be suspended due to no subscribers
    pub async fn maybe_suspend_idle_session(&self, session_id: &str) -> Result<()> {
        // Check if session has no subscribers
        let should_suspend = {
            let sessions = self.active_sessions.read().await;
            if let Some(managed_session) = sessions.get(session_id) {
                managed_session.subscriber_count == 0
            } else {
                false // Already suspended or deleted
            }
        };

        if should_suspend {
            info!(session_id = %session_id, "No active subscribers, suspending session");
            self.suspend_session(session_id).await?;
        }

        Ok(())
    }

    /// Get session state for gRPC responses
    pub async fn get_session_state(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::session::SessionState>> {
        info!("get_session_state called for session: {}", session_id);

        // Always load from store to get the most up-to-date state
        // The in-memory state in ManagedSession may be stale
        match self.store.get_session(session_id).await {
            Ok(Some(session)) => {
                info!(
                    "Loaded session from store with {} messages",
                    session.state.messages.len()
                );
                Ok(Some(session.state))
            }
            Ok(None) => {
                info!("Session not found in store: {}", session_id);
                Ok(None)
            }
            Err(e) => {
                error!("Error loading session from store: {}", e);
                Err(SessionManagerError::Storage(e).into())
            }
        }
    }
}

/// Convert AppEvent to StreamEvent, returning None for events that shouldn't be streamed
fn translate_app_event(app_event: AppEvent, _session_id: &str) -> Option<StreamEvent> {
    match app_event {
        AppEvent::MessageAdded { message, model } => Some(StreamEvent::MessageComplete {
            message,
            usage: None,
            metadata: std::collections::HashMap::new(),
            model,
        }),

        AppEvent::MessagePart { id, delta } => Some(StreamEvent::MessagePart {
            content: delta,
            message_id: id,
        }),

        AppEvent::ToolCallStarted { name, id, model } => {
            let tool_call = ToolCall {
                id: id.clone(),
                name: name.clone(),
                parameters: serde_json::Value::Null, // We don't have parameters in this event
            };
            Some(StreamEvent::ToolCallStarted {
                tool_call,
                metadata: std::collections::HashMap::new(),
                model,
            })
        }

        AppEvent::ToolCallCompleted {
            name: _,
            result,
            id,
            model,
        } => Some(StreamEvent::ToolCallCompleted {
            tool_call_id: id,
            result,
            metadata: std::collections::HashMap::new(),
            model,
        }),

        AppEvent::ToolCallFailed {
            name: _,
            error,
            id,
            model,
        } => Some(StreamEvent::ToolCallFailed {
            tool_call_id: id,
            error,
            metadata: std::collections::HashMap::new(),
            model,
        }),

        AppEvent::WorkspaceChanged => Some(StreamEvent::WorkspaceChanged),

        AppEvent::WorkspaceFiles { files } => Some(StreamEvent::WorkspaceFiles {
            files: files.clone(),
        }),

        // These events don't need to be streamed
        _ => None,
    }
}
/// Update session state based on a StreamEvent
async fn update_session_state_for_event(
    store: &Arc<dyn SessionStore>,
    session_id: &str,
    event: &StreamEvent,
) -> Result<()> {
    match event {
        StreamEvent::MessageComplete { message, .. } => {
            store.append_message(session_id, message).await?;
        }
        StreamEvent::ToolCallStarted { tool_call, .. } => {
            store.create_tool_call(session_id, tool_call).await?;
        }
        StreamEvent::ToolCallCompleted {
            tool_call_id,
            result,
            ..
        } => {
            let stats = crate::session::ToolExecutionStats::success_typed(
                serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
                result.variant_name().to_string(),
                0,
            );
            let update = ToolCallUpdate::set_result(stats);
            store.update_tool_call(tool_call_id, update).await?;

            // Also add a Tool message with the result
            // Get thread info from the latest message in the session
            let messages = store.get_messages(session_id, None).await?;
            let (thread_id, parent_id) = if let Some(last_msg) = messages.last() {
                (*last_msg.thread_id(), Some(last_msg.id().to_string()))
            } else {
                // Fallback for empty session
                (uuid::Uuid::now_v7(), None)
            };

            let tool_message = ConversationMessage::Tool {
                tool_use_id: tool_call_id.clone(),
                result: result.clone(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("Time went backwards")
                    .as_secs(),
                id: format!("tool_result_{tool_call_id}"),
                thread_id,
                parent_message_id: parent_id,
            };
            store.append_message(session_id, &tool_message).await?;
        }
        StreamEvent::ToolCallFailed {
            tool_call_id,
            error,
            ..
        } => {
            let update = ToolCallUpdate::set_error(error.clone());
            store.update_tool_call(tool_call_id, update).await?;

            // Also add a Tool message with the error
            // Get thread info from the latest message in the session
            let messages = store.get_messages(session_id, None).await?;
            let (thread_id, parent_id) = if let Some(last_msg) = messages.last() {
                (*last_msg.thread_id(), Some(last_msg.id().to_string()))
            } else {
                // Fallback for empty session
                (uuid::Uuid::now_v7(), None)
            };

            let tool_error = conductor_tools::error::ToolError::Execution {
                tool_name: "unknown".to_string(), // We don't have the tool name here
                message: error.clone(),
            };
            let tool_message = ConversationMessage::Tool {
                tool_use_id: tool_call_id.clone(),
                result: crate::app::conversation::ToolResult::Error(tool_error),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("Time went backwards")
                    .as_secs(),
                id: format!("tool_result_{tool_call_id}"),
                thread_id,
                parent_message_id: parent_id,
            };
            store.append_message(session_id, &tool_message).await?;
        }
        // Other events don't directly modify stored state
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::conversation::{AssistantContent, Role, UserContent};
    use crate::config::LlmConfig;
    use crate::session::stores::sqlite::SqliteSessionStore;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    async fn create_test_manager() -> (SessionManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteSessionStore::new(&db_path).await.unwrap());

        let (event_tx, _event_rx) = mpsc::channel(100);
        let config = SessionManagerConfig {
            max_concurrent_sessions: 100,
            default_model: Model::default(),
            auto_persist: true,
        };
        let manager = SessionManager::new(store, config, event_tx);

        (manager, temp_dir)
    }

    fn create_test_app_config() -> AppConfig {
        AppConfig {
            llm_config: LlmConfig {
                anthropic_api_key: None,
                openai_api_key: None,
                gemini_api_key: None,
            },
        }
    }

    #[tokio::test]
    async fn test_create_and_resume_session() {
        let (manager, temp) = create_test_manager().await;
        let app_config = create_test_app_config();

        // Create session
        let session_config = SessionConfig {
            workspace: crate::session::state::WorkspaceConfig::Local {
                path: temp.path().to_path_buf(),
            },
            tool_config: crate::session::SessionToolConfig::default(),
            system_prompt: None,
            metadata: std::collections::HashMap::new(),
        };
        let (session_id, _command_tx) = manager
            .create_session(session_config, app_config.clone())
            .await
            .unwrap();
        assert!(!session_id.is_empty());
        assert!(manager.is_session_active(&session_id).await);

        // Suspend session
        assert!(manager.suspend_session(&session_id).await.unwrap());
        assert!(!manager.is_session_active(&session_id).await);

        // Resume Session
        let _command_tx = manager
            .resume_session(&session_id, app_config)
            .await
            .unwrap();
        assert!(manager.is_session_active(&session_id).await);
    }

    #[tokio::test]
    async fn test_session_cleanup() {
        let (manager, temp) = create_test_manager().await;
        let app_config = create_test_app_config();

        // Create session
        let session_config = SessionConfig {
            workspace: crate::session::state::WorkspaceConfig::Local {
                path: temp.path().to_path_buf(),
            },
            tool_config: crate::session::SessionToolConfig::default(),
            system_prompt: None,
            metadata: std::collections::HashMap::new(),
        };
        let (session_id, _command_tx) = manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // Make session appear inactive by manipulating last_activity
        {
            let mut sessions = manager.active_sessions.write().await;
            if let Some(session) = sessions.get_mut(&session_id) {
                session.last_activity = chrono::Utc::now() - chrono::Duration::hours(2);
                session.subscriber_count = 0;
            }
        }

        // Cleanup should suspend the session
        let cleaned = manager
            .cleanup_inactive_sessions(chrono::Duration::hours(1))
            .await;
        assert_eq!(cleaned, 1);
        assert!(!manager.is_session_active(&session_id).await);
    }

    #[tokio::test]
    async fn test_capacity_rejection() {
        let temp_dir = TempDir::new().unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteSessionStore::new(&db_path).await.unwrap());

        let (event_tx, _event_rx) = mpsc::channel(100);
        let config = SessionManagerConfig {
            max_concurrent_sessions: 1, // Set to 1 for testing
            default_model: Model::default(),
            auto_persist: true,
        };
        let manager = SessionManager::new(store, config, event_tx);
        let app_config = create_test_app_config();

        // Create first session - should succeed
        let tool_config = crate::session::SessionToolConfig {
            approval_policy: crate::session::ToolApprovalPolicy::AlwaysAsk,
            ..Default::default()
        };

        let session_config = SessionConfig {
            workspace: crate::session::state::WorkspaceConfig::Local {
                path: temp.path().to_path_buf(),
            },
            tool_config,
            system_prompt: None,
            metadata: std::collections::HashMap::new(),
        };
        let (session_id1, _command_tx) = manager
            .create_session(session_config.clone(), app_config.clone())
            .await
            .unwrap();
        assert!(!session_id1.is_empty());

        // Create second session - should fail with capacity error
        let result = manager.create_session(session_config, app_config).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(crate::error::Error::SessionManager(
                SessionManagerError::CapacityExceeded { .. }
            ))
        ));
        match result.unwrap_err() {
            crate::error::Error::SessionManager(SessionManagerError::CapacityExceeded {
                current,
                max,
            }) => {
                assert_eq!(current, 1);
                assert_eq!(max, 1);
            }
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn test_tool_result_persistence_on_resume() {
        let (manager, temp) = create_test_manager().await;
        let app_config = create_test_app_config();

        // Create session
        let session_config = SessionConfig {
            workspace: crate::session::state::WorkspaceConfig::Local {
                path: temp.path().to_path_buf(),
            },
            tool_config: crate::session::SessionToolConfig::default(),
            system_prompt: None,
            metadata: std::collections::HashMap::new(),
        };
        let (session_id, _command_tx) = manager
            .create_session(session_config, app_config.clone())
            .await
            .unwrap();

        // Simulate adding messages including tool calls and results
        // First, add a user message
        let thread_id = uuid::Uuid::now_v7();
        let user_message = ConversationMessage::User {
            content: vec![UserContent::Text {
                text: "Read the file test.txt".to_string(),
            }],
            timestamp: 123456789,
            id: "user_1".to_string(),
            thread_id,
            parent_message_id: None,
        };
        manager
            .store
            .append_message(&session_id, &user_message)
            .await
            .unwrap();

        // Add an assistant message with a tool call
        let assistant_message = ConversationMessage::Assistant {
            content: vec![
                AssistantContent::Text {
                    text: "I'll read that file for you.".to_string(),
                },
                AssistantContent::ToolCall {
                    tool_call: ToolCall {
                        id: "tool_call_1".to_string(),
                        name: "read_file".to_string(),
                        parameters: serde_json::json!({"path": "test.txt"}),
                    },
                },
            ],
            timestamp: 123456790,
            id: "assistant_1".to_string(),
            thread_id,
            parent_message_id: Some("user_1".to_string()),
        };
        manager
            .store
            .append_message(&session_id, &assistant_message)
            .await
            .unwrap();

        // Simulate tool call events
        let tool_call_started = StreamEvent::ToolCallStarted {
            tool_call: ToolCall {
                id: "tool_call_1".to_string(),
                name: "read_file".to_string(),
                parameters: serde_json::json!({"path": "test.txt"}),
            },
            metadata: std::collections::HashMap::new(),
            model: Model::Claude3_5Sonnet20241022,
        };

        let tool_call_completed = StreamEvent::ToolCallCompleted {
            tool_call_id: "tool_call_1".to_string(),
            result: crate::app::conversation::ToolResult::FileContent(
                conductor_tools::result::FileContentResult {
                    content: "File contents: Hello, world!".to_string(),
                    file_path: "test.txt".to_string(),
                    line_count: 1,
                    truncated: false,
                },
            ),
            metadata: std::collections::HashMap::new(),
            model: Model::Claude3_5Sonnet20241022,
        };

        // Process the events through update_session_state_for_event
        update_session_state_for_event(&manager.store, &session_id, &tool_call_started)
            .await
            .unwrap();
        update_session_state_for_event(&manager.store, &session_id, &tool_call_completed)
            .await
            .unwrap();

        // Add a follow-up assistant message
        let followup_message = ConversationMessage::Assistant {
            content: vec![AssistantContent::Text {
                text: "The file contains: Hello, world!".to_string(),
            }],
            timestamp: 123456791,
            id: "assistant_2".to_string(),
            thread_id,
            parent_message_id: Some("assistant_1".to_string()),
        };
        manager
            .store
            .append_message(&session_id, &followup_message)
            .await
            .unwrap();

        // Suspend the session
        manager.suspend_session(&session_id).await.unwrap();

        // Load the session from storage and verify tool result message exists
        let loaded_session = manager
            .store
            .get_session(&session_id)
            .await
            .unwrap()
            .unwrap();

        // Should have 4 messages: user, assistant with tool call, tool result, followup
        assert_eq!(loaded_session.state.messages.len(), 4);

        // Verify the tool result message exists and has correct content
        let tool_result_msg = &loaded_session.state.messages[2];
        assert_eq!(tool_result_msg.role(), Role::Tool);

        // Verify the content structure
        assert!(matches!(tool_result_msg, ConversationMessage::Tool { .. }));
        match tool_result_msg {
            ConversationMessage::Tool {
                tool_use_id,
                result,
                ..
            } => {
                assert_eq!(tool_use_id, "tool_call_1");
                assert!(matches!(
                    result,
                    crate::app::conversation::ToolResult::FileContent(_)
                ));
                match result {
                    crate::app::conversation::ToolResult::FileContent(content) => {
                        assert!(content.content.contains("Hello, world!"));
                    }
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }

        // Now test resuming the session - it should work without API errors
        let _command_tx = manager
            .resume_session(&session_id, app_config)
            .await
            .unwrap();

        // The session should be properly restored with all messages including tool results
        // If the bug were still present, trying to send a new message would fail with the
        // "tool_use ids were found without tool_result blocks" error from the API
    }
}
