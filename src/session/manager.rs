use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid;

use crate::api::{Message as ApiMessage, Model, ToolCall as ApiToolCall};
use crate::app::{App, AppCommand, AppConfig, AppEvent};
use crate::events::{StreamEvent, StreamEventWithMetadata};
use crate::session::{
    Session, SessionConfig, SessionFilter, SessionInfo, SessionState, SessionStore,
    SessionStoreError, ToolCallUpdate, ToolResult,
};

/// Session manager specific errors
#[derive(Debug, Error)]
pub enum SessionManagerError {
    #[error("Maximum session capacity reached ({current}/{max}). Cannot create new session.")]
    CapacityExceeded { current: usize, max: usize },

    #[error("Session not active: {session_id}")]
    SessionNotActive { session_id: String },

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
    pub fn new(
        session: Session,
        app_config: AppConfig,
        event_tx: mpsc::Sender<StreamEvent>,
        store: Arc<dyn SessionStore>,
        global_event_tx: mpsc::Sender<StreamEventWithMetadata>,
        default_model: Model,
    ) -> Result<Self> {
        // Create channels for the App
        let (app_event_tx, mut app_event_rx) = mpsc::channel(100);
        let (app_command_tx, app_command_rx) = mpsc::channel::<AppCommand>(32);

        // Always create external event channel
        let (external_event_tx, external_event_rx) = mpsc::channel(100);

        // Initialize the global command sender for tool approval requests
        crate::app::OpContext::init_command_tx(app_command_tx.clone());

        // Create the App instance
        let mut app = App::new(app_config, app_event_tx, default_model)?;

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
                        // Update session state
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
    ) -> Result<(String, mpsc::Sender<AppCommand>), SessionManagerError> {
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
                });
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
        )
        .map_err(|e| SessionManagerError::CreationFailed {
            message: format!("Failed to create managed session: {}", e),
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
    pub async fn take_event_receiver(&self, session_id: &str) -> Option<mpsc::Receiver<AppEvent>> {
        let mut sessions = self.active_sessions.write().await;
        sessions.get_mut(session_id)?.take_event_rx()
    }

    /// Get session information
    pub async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionInfo>, SessionStoreError> {
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

    /// Resume a session (load from storage and activate)
    pub async fn resume_session(
        &self,
        session_id: &str,
        app_config: AppConfig,
    ) -> Result<(bool, mpsc::Sender<AppCommand>), SessionManagerError> {
        // Check if already active
        {
            let sessions = self.active_sessions.read().await;
            if let Some(managed_session) = sessions.get(session_id) {
                debug!(session_id = %session_id, "Session already active");
                return Ok((true, managed_session.command_tx.clone()));
            }
        }

        // Load from store
        let session = match self.store.get_session(session_id).await? {
            Some(session) => session,
            None => {
                debug!(session_id = %session_id, "Session not found in store");
                return Err(SessionManagerError::SessionNotActive {
                    session_id: session_id.to_string(),
                });
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

        // Create managed session with event translation
        let managed_session = ManagedSession::new(
            session.clone(),
            app_config,
            event_tx,
            self.store.clone(),
            self.event_tx.clone(),
            self.config.default_model,
        )
        .map_err(|e| SessionManagerError::CreationFailed {
            message: format!("Failed to create managed session: {}", e),
        })?;

        // Get command sender before restoration
        let command_tx = managed_session.command_tx.clone();

        // Restore conversation history
        if !session.state.messages.is_empty() {
            info!(session_id = %session_id, message_count = session.state.messages.len(), "Restoring conversation history");

            // Send messages to the App to rebuild conversation state
            for message in &session.state.messages {
                // Convert from stored format to App format
                let app_message = crate::app::Message::try_from(message.clone()).map_err(|e| {
                    SessionManagerError::CreationFailed {
                        message: format!("Failed to restore message: {}", e),
                    }
                })?;

                // Send command to add message to conversation
                command_tx
                    .send(AppCommand::RestoreMessage(app_message))
                    .await
                    .map_err(|_| SessionManagerError::CreationFailed {
                        message: "Failed to send restore command to App".to_string(),
                    })?;
            }
        }

        // Restore approved tools
        if !session.state.approved_tools.is_empty() {
            info!(session_id = %session_id, tool_count = session.state.approved_tools.len(), "Restoring approved tools");

            // Send all approved tools at once
            let tools: Vec<String> = session.state.approved_tools.iter().cloned().collect();
            command_tx
                .send(AppCommand::PreApproveTools(tools))
                .await
                .map_err(|_| SessionManagerError::CreationFailed {
                    message: "Failed to send tool approval command to App".to_string(),
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
        Ok((true, command_tx))
    }

    /// Suspend a session (save to storage and deactivate)
    pub async fn suspend_session(&self, session_id: &str) -> Result<bool, SessionStoreError> {
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
    pub async fn delete_session(&self, session_id: &str) -> Result<bool, SessionStoreError> {
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
    pub async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionInfo>, SessionStoreError> {
        self.store.list_sessions(filter).await
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
    pub async fn send_command(
        &self,
        session_id: &str,
        command: AppCommand,
    ) -> Result<(), SessionManagerError> {
        let sessions = self.active_sessions.read().await;
        if let Some(managed_session) = sessions.get(session_id) {
            managed_session.command_tx.send(command).await.map_err(|_| {
                SessionManagerError::SessionNotActive {
                    session_id: session_id.to_string(),
                }
            })
        } else {
            Err(SessionManagerError::SessionNotActive {
                session_id: session_id.to_string(),
            })
        }
    }

    /// Update session state and persist if auto-persist is enabled
    pub async fn update_session_state(
        &self,
        session_id: &str,
        update_fn: impl FnOnce(&mut SessionState),
    ) -> Result<(), SessionManagerError> {
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
                });
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
}

/// Event translation loop that converts AppEvents to StreamEvents
async fn event_translation_loop(
    session_id: String,
    mut app_event_rx: mpsc::Receiver<AppEvent>,
    store: Arc<dyn SessionStore>,
    global_event_tx: mpsc::Sender<StreamEventWithMetadata>,
) {
    info!(session_id = %session_id, "Starting event translation loop");

    while let Some(app_event) = app_event_rx.recv().await {
        debug!(session_id = %session_id, "Translating app event: {:?}", app_event);

        // Translate AppEvent to StreamEvent
        let stream_event = match translate_app_event(app_event, &session_id) {
            Some(event) => event,
            None => continue, // Skip events that don't need to be streamed
        };

        // Persist event and get sequence number
        let sequence_num = match store.append_event(&session_id, &stream_event).await {
            Ok(seq) => seq,
            Err(e) => {
                error!(session_id = %session_id, error = %e, "Failed to persist event");
                continue;
            }
        };

        // Update session state based on the event
        if let Err(e) = update_session_state_for_event(&store, &session_id, &stream_event).await {
            error!(session_id = %session_id, error = %e, "Failed to update session state");
        }

        // Broadcast to subscribers
        let event_with_metadata =
            StreamEventWithMetadata::new(sequence_num, session_id.clone(), stream_event);
        if let Err(e) = global_event_tx.try_send(event_with_metadata) {
            warn!(session_id = %session_id, error = %e, "Failed to broadcast event");
        }
    }

    info!(session_id = %session_id, "Event translation loop ended");
}

/// Convert AppEvent to StreamEvent, returning None for events that shouldn't be streamed
fn translate_app_event(app_event: AppEvent, _session_id: &str) -> Option<StreamEvent> {
    use crate::api::messages::{MessageContent, MessageRole};
    use crate::app::conversation::Role;

    match app_event {
        AppEvent::MessageAdded {
            role,
            content_blocks,
            id,
            model,
        } => {
            // Convert role
            let api_role = match role {
                Role::User => MessageRole::User,
                Role::Assistant => MessageRole::Assistant,
                Role::Tool => MessageRole::Tool,
            };

            // Convert content blocks
            let message_content = if content_blocks.len() == 1 {
                if let Some(block) = content_blocks.first() {
                    match block {
                        crate::app::MessageContentBlock::Text(content) => MessageContent::Text {
                            content: content.clone(),
                        },
                        crate::app::MessageContentBlock::ToolCall(tool_call) => {
                            MessageContent::StructuredContent {
                                content: crate::api::messages::StructuredContent(vec![
                                    crate::api::messages::ContentBlock::ToolUse {
                                        id: tool_call.id.clone(),
                                        name: tool_call.name.clone(),
                                        input: tool_call.parameters.clone(),
                                    },
                                ]),
                            }
                        }
                        crate::app::MessageContentBlock::ToolResult {
                            tool_use_id,
                            result,
                        } => MessageContent::StructuredContent {
                            content: crate::api::messages::StructuredContent(vec![
                                crate::api::messages::ContentBlock::ToolResult {
                                    tool_use_id: tool_use_id.clone(),
                                    content: vec![crate::api::messages::ContentBlock::Text {
                                        text: result.clone(),
                                    }],
                                    is_error: None,
                                },
                            ]),
                        },
                    }
                } else {
                    MessageContent::Text {
                        content: String::new(),
                    }
                }
            } else {
                // Multiple content blocks - convert to structured content
                let api_blocks: Vec<crate::api::messages::ContentBlock> = content_blocks
                    .into_iter()
                    .map(|block| match block {
                        crate::app::MessageContentBlock::Text(content) => {
                            crate::api::messages::ContentBlock::Text { text: content }
                        }
                        crate::app::MessageContentBlock::ToolCall(tool_call) => {
                            crate::api::messages::ContentBlock::ToolUse {
                                id: tool_call.id,
                                name: tool_call.name,
                                input: tool_call.parameters,
                            }
                        }
                        crate::app::MessageContentBlock::ToolResult {
                            tool_use_id,
                            result,
                        } => crate::api::messages::ContentBlock::ToolResult {
                            tool_use_id,
                            content: vec![crate::api::messages::ContentBlock::Text {
                                text: result,
                            }],
                            is_error: None,
                        },
                    })
                    .collect();

                MessageContent::StructuredContent {
                    content: crate::api::messages::StructuredContent(api_blocks),
                }
            };

            let message = ApiMessage {
                id: Some(id),
                role: api_role,
                content: message_content,
            };

            Some(StreamEvent::MessageComplete {
                message,
                usage: None,
                metadata: std::collections::HashMap::new(),
                model,
            })
        }

        AppEvent::MessagePart { id, delta } => Some(StreamEvent::MessagePart {
            content: delta,
            message_id: id,
        }),

        AppEvent::ToolCallStarted { name, id, model } => {
            let tool_call = ApiToolCall {
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
            name: _name,
            result,
            id,
            model,
        } => {
            Some(StreamEvent::ToolCallCompleted {
                tool_call_id: id,
                result: ToolResult::success(result, 0), // TODO: Add proper timing
                metadata: std::collections::HashMap::new(),
                model,
            })
        }

        AppEvent::ToolCallFailed {
            name: _name,
            error,
            id,
            model,
        } => Some(StreamEvent::ToolCallFailed {
            tool_call_id: id,
            error,
            metadata: std::collections::HashMap::new(),
            model,
        }),

        AppEvent::RequestToolApproval {
            name,
            parameters,
            id,
        } => {
            let tool_call = ApiToolCall {
                id: id.clone(),
                name: name.clone(),
                parameters,
            };
            Some(StreamEvent::ToolApprovalRequired {
                tool_call,
                timeout_ms: None,
                metadata: std::collections::HashMap::new(),
            })
        }

        // These events don't need to be translated to StreamEvents
        AppEvent::MessageUpdated { .. } => None, // Internal state update
        AppEvent::ThinkingStarted => Some(StreamEvent::OperationStarted {
            operation_id: format!("op_{}", uuid::Uuid::new_v4()),
        }),
        AppEvent::ThinkingCompleted => Some(StreamEvent::OperationCompleted {
            operation_id: format!("op_{}", uuid::Uuid::new_v4()),
        }),
        AppEvent::CommandResponse { .. } => None, // System messages, not persisted
        AppEvent::OperationCancelled { info } => Some(StreamEvent::OperationCancelled {
            operation_id: format!("op_{}", uuid::Uuid::new_v4()),
            reason: format!("Operation cancelled: {:?}", info),
        }),
        AppEvent::ModelChanged { .. } => None, // Internal configuration change
        AppEvent::Error { message } => Some(StreamEvent::Error {
            message,
            error_type: crate::events::ErrorType::Internal,
        }),
        AppEvent::RestoredMessage { .. } => None, // Already persisted, don't double-log
    }
}

/// Update session state based on a StreamEvent
async fn update_session_state_for_event(
    store: &Arc<dyn SessionStore>,
    session_id: &str,
    event: &StreamEvent,
) -> Result<(), SessionStoreError> {
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
            let update = ToolCallUpdate::set_result(result.clone());
            store.update_tool_call(tool_call_id, update).await?;
        }
        StreamEvent::ToolCallFailed {
            tool_call_id,
            error,
            ..
        } => {
            let update = ToolCallUpdate::set_error(error.clone());
            store.update_tool_call(tool_call_id, update).await?;
        }
        // Other events don't directly modify stored state
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            default_model: Model::Claude3_5Sonnet20241022,
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
        let (manager, _temp) = create_test_manager().await;
        let app_config = create_test_app_config();

        // Create session
        let session_config = SessionConfig {
            tool_policy: crate::session::ToolApprovalPolicy::AlwaysAsk,
            tool_config: crate::session::SessionToolConfig::default(),
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

        // Resume session
        let (resumed, _command_tx) = manager
            .resume_session(&session_id, app_config)
            .await
            .unwrap();
        assert!(resumed);
        assert!(manager.is_session_active(&session_id).await);
    }

    #[tokio::test]
    async fn test_session_cleanup() {
        let (manager, _temp) = create_test_manager().await;
        let app_config = create_test_app_config();

        // Create session
        let session_config = SessionConfig {
            tool_policy: crate::session::ToolApprovalPolicy::AlwaysAsk,
            tool_config: crate::session::SessionToolConfig::default(),
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
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteSessionStore::new(&db_path).await.unwrap());

        let (event_tx, _event_rx) = mpsc::channel(100);
        let config = SessionManagerConfig {
            max_concurrent_sessions: 1, // Set to 1 for testing
            default_model: Model::Claude3_5Sonnet20241022,
            auto_persist: true,
        };
        let manager = SessionManager::new(store, config, event_tx);
        let app_config = create_test_app_config();

        // Create first session - should succeed
        let session_config = SessionConfig {
            tool_policy: crate::session::ToolApprovalPolicy::AlwaysAsk,
            tool_config: crate::session::SessionToolConfig::default(),
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
        match result.unwrap_err() {
            SessionManagerError::CapacityExceeded { current, max } => {
                assert_eq!(current, 1);
                assert_eq!(max, 1);
            }
            _ => panic!("Expected CapacityExceeded error"),
        }
    }
}
