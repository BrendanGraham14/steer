use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::api::Client as ApiClient;
use crate::app::conversation::UserContent;
use crate::app::domain::action::Action;
use crate::app::domain::delta::StreamDelta;
use crate::app::domain::event::SessionEvent;
use crate::app::domain::reduce::apply_event_to_state;
use crate::app::domain::session::EventStore;
use crate::app::domain::state::AppState;
use crate::app::domain::types::{MessageId, OpId, RequestId, SessionId};

use crate::config::model::ModelId;
use crate::primary_agents::{default_primary_agent_id, resolve_effective_config};
use crate::prompts::system_prompt_for_model;
use crate::session::state::SessionConfig;
use crate::tools::ToolExecutor;
use tracing::warn;

use super::session_actor::{SessionActorHandle, SessionError, spawn_session_actor};
use super::subscription::SessionEventSubscription;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("Session already exists: {session_id}")]
    SessionAlreadyExists { session_id: String },

    #[error("Session error: {0}")]
    Session(SessionError),

    #[error("Event store error: {0}")]
    EventStore(#[from] crate::app::domain::session::EventStoreError),

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Invalid input: {message}")]
    InvalidInput { message: String },

    #[error("Supervisor shutting down")]
    ShuttingDown,
}

impl From<SessionError> for RuntimeError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::InvalidInput { message, .. } => RuntimeError::InvalidInput { message },
            other => RuntimeError::Session(other),
        }
    }
}

pub(crate) enum SupervisorCmd {
    CreateSession {
        config: Box<SessionConfig>,
        reply: oneshot::Sender<Result<SessionId, RuntimeError>>,
    },
    ResumeSession {
        session_id: SessionId,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    SuspendSession {
        session_id: SessionId,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    DeleteSession {
        session_id: SessionId,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    DispatchAction {
        session_id: SessionId,
        action: Box<Action>,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    SubscribeEvents {
        session_id: SessionId,
        reply: oneshot::Sender<Result<SessionEventSubscription, RuntimeError>>,
    },
    SubscribeDeltas {
        session_id: SessionId,
        reply: oneshot::Sender<Result<broadcast::Receiver<StreamDelta>, RuntimeError>>,
    },
    LoadEventsAfter {
        session_id: SessionId,
        after_seq: u64,
        reply: oneshot::Sender<Result<Vec<(u64, SessionEvent)>, RuntimeError>>,
    },
    GetSessionState {
        session_id: SessionId,
        reply: oneshot::Sender<Result<AppState, RuntimeError>>,
    },
    IsSessionActive {
        session_id: SessionId,
        reply: oneshot::Sender<bool>,
    },
    ListActiveSessions {
        reply: oneshot::Sender<Vec<SessionId>>,
    },
    ListAllSessions {
        reply: oneshot::Sender<Result<Vec<SessionId>, RuntimeError>>,
    },
    SessionExists {
        session_id: SessionId,
        reply: oneshot::Sender<Result<bool, RuntimeError>>,
    },
    Shutdown,
}

struct RuntimeSupervisor {
    sessions: HashMap<SessionId, SessionActorHandle>,
    event_store: Arc<dyn EventStore>,
    api_client: Arc<ApiClient>,
    tool_executor: Arc<ToolExecutor>,
}

impl RuntimeSupervisor {
    fn new(
        event_store: Arc<dyn EventStore>,
        api_client: Arc<ApiClient>,
        tool_executor: Arc<ToolExecutor>,
    ) -> Self {
        Self {
            sessions: HashMap::new(),
            event_store,
            api_client,
            tool_executor,
        }
    }

    async fn run(mut self, mut cmd_rx: mpsc::Receiver<SupervisorCmd>) {
        loop {
            tokio::select! {
                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        SupervisorCmd::CreateSession { config, reply } => {
                            let result = self.create_session(*config).await;
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::ResumeSession { session_id, reply } => {
                            let result = self.resume_session(session_id).await;
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::SuspendSession { session_id, reply } => {
                            let result = self.suspend_session(session_id).await;
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::DeleteSession { session_id, reply } => {
                            let result = self.delete_session(session_id).await;
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::DispatchAction { session_id, action, reply } => {
                            let result = self.dispatch_action(session_id, *action).await;
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::SubscribeEvents { session_id, reply } => {
                            let result = self.subscribe_events(session_id).await;
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::SubscribeDeltas { session_id, reply } => {
                            let result = self.subscribe_deltas(session_id).await;
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::LoadEventsAfter {
                            session_id,
                            after_seq,
                            reply,
                        } => {
                            let result = self
                                .event_store
                                .load_events_after(session_id, after_seq)
                                .await
                                .map_err(RuntimeError::from);
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::GetSessionState { session_id, reply } => {
                            let result = self.get_session_state(session_id).await;
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::IsSessionActive { session_id, reply } => {
                            let is_active = self.sessions.contains_key(&session_id);
                            let _ = reply.send(is_active);
                        }
                        SupervisorCmd::ListActiveSessions { reply } => {
                            let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
                            let _ = reply.send(sessions);
                        }
                        SupervisorCmd::ListAllSessions { reply } => {
                            let result = self.event_store.list_session_ids().await
                                .map_err(RuntimeError::from);
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::SessionExists { session_id, reply } => {
                            let result = self.event_store.session_exists(session_id).await
                                .map_err(RuntimeError::from);
                            let _ = reply.send(result);
                        }
                        SupervisorCmd::Shutdown => {
                            self.shutdown_all().await;
                            break;
                        }
                    }
                }
                else => break,
            }
        }

        tracing::info!("Runtime supervisor stopped");
    }

    async fn create_session(&mut self, config: SessionConfig) -> Result<SessionId, RuntimeError> {
        let session_id = SessionId::new();

        self.event_store.create_session(session_id).await?;

        let mut config = config;

        if config.primary_agent_id.is_none() {
            config.primary_agent_id = Some(default_primary_agent_id().to_string());
        }
        let mut config = resolve_effective_config(&config);

        let system_context = self.resolve_system_context(&config).await;
        if let Some(context) = &system_context {
            config.system_prompt = Some(context.prompt.clone());
        }

        let session_created_event = SessionEvent::SessionCreated {
            config: Box::new(config.clone()),
            metadata: config.metadata.clone(),
            parent_session_id: None,
        };
        self.event_store
            .append(session_id, &session_created_event)
            .await?;

        let mut state = AppState::new(session_id);
        state.apply_session_config(&config, config.primary_agent_id.clone(), true);
        if let Some(system_context) = system_context {
            state.cached_system_context = Some(system_context);
        }

        let handle = spawn_session_actor(
            session_id,
            state,
            self.event_store.clone(),
            self.api_client.clone(),
            self.tool_executor.clone(),
        );
        self.sessions.insert(session_id, handle);

        tracing::info!(session_id = %session_id, "Created session");

        Ok(session_id)
    }

    async fn resume_session(&mut self, session_id: SessionId) -> Result<(), RuntimeError> {
        if self.sessions.contains_key(&session_id) {
            return Ok(());
        }

        if !self.event_store.session_exists(session_id).await? {
            return Err(RuntimeError::SessionNotFound {
                session_id: session_id.to_string(),
            });
        }

        let events = self.event_store.load_events(session_id).await?;

        let mut state = AppState::new(session_id);
        for (_, event) in &events {
            apply_event_to_state(&mut state, event);
        }

        if let Some(config) = state.session_config.clone() {
            let mut resolved = resolve_effective_config(&config);
            let system_context = self.resolve_system_context(&resolved).await;
            if let Some(context) = &system_context {
                resolved.system_prompt = Some(context.prompt.clone());
            }
            state.apply_session_config(&resolved, resolved.primary_agent_id.clone(), false);
            state.cached_system_context = system_context;
        }

        let should_drain_queue = !state.has_active_operation() && !state.queued_work.is_empty();
        let handle = spawn_session_actor(
            session_id,
            state,
            self.event_store.clone(),
            self.api_client.clone(),
            self.tool_executor.clone(),
        );

        if should_drain_queue {
            handle
                .dispatch(Action::DrainQueuedWork { session_id })
                .await?;
        }

        self.sessions.insert(session_id, handle);

        tracing::info!(
            session_id = %session_id,
            event_count = events.len(),
            "Resumed session"
        );

        Ok(())
    }

    async fn resolve_system_context(
        &self,
        config: &SessionConfig,
    ) -> Option<crate::app::SystemContext> {
        let prompt = config
            .system_prompt
            .as_ref()
            .and_then(|prompt| {
                if prompt.trim().is_empty() {
                    None
                } else {
                    Some(prompt.clone())
                }
            })
            .unwrap_or_else(|| system_prompt_for_model(&config.default_model));

        let workspace = match self.tool_executor.workspace() {
            Some(workspace) => workspace,
            None => return Some(crate::app::SystemContext::new(prompt)),
        };

        let environment = match workspace.environment().await {
            Ok(env_info) => Some(env_info),
            Err(error) => {
                warn!(error = %error, "Failed to collect environment info for system context");
                None
            }
        };

        Some(crate::app::SystemContext::with_environment(
            prompt,
            environment,
        ))
    }

    async fn suspend_session(&mut self, session_id: SessionId) -> Result<(), RuntimeError> {
        if let Some(handle) = self.sessions.remove(&session_id) {
            let _ = handle.suspend().await;
            tracing::info!(session_id = %session_id, "Suspended session");
        }
        Ok(())
    }

    async fn delete_session(&mut self, session_id: SessionId) -> Result<(), RuntimeError> {
        if let Some(handle) = self.sessions.remove(&session_id) {
            handle.shutdown();
        }

        self.event_store.delete_session(session_id).await?;

        tracing::info!(session_id = %session_id, "Deleted session");

        Ok(())
    }

    async fn dispatch_action(
        &mut self,
        session_id: SessionId,
        action: Action,
    ) -> Result<(), RuntimeError> {
        if !self.sessions.contains_key(&session_id) {
            self.resume_session(session_id).await?;
        }

        let handle =
            self.sessions
                .get(&session_id)
                .ok_or_else(|| RuntimeError::SessionNotFound {
                    session_id: session_id.to_string(),
                })?;

        handle.dispatch(action).await?;

        Ok(())
    }

    async fn subscribe_events(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionEventSubscription, RuntimeError> {
        if !self.sessions.contains_key(&session_id) {
            self.resume_session(session_id).await?;
        }

        let handle =
            self.sessions
                .get(&session_id)
                .ok_or_else(|| RuntimeError::SessionNotFound {
                    session_id: session_id.to_string(),
                })?;

        let subscription = handle.subscribe().await?;

        Ok(subscription)
    }

    async fn subscribe_deltas(
        &mut self,
        session_id: SessionId,
    ) -> Result<broadcast::Receiver<StreamDelta>, RuntimeError> {
        if !self.sessions.contains_key(&session_id) {
            self.resume_session(session_id).await?;
        }

        let handle =
            self.sessions
                .get(&session_id)
                .ok_or_else(|| RuntimeError::SessionNotFound {
                    session_id: session_id.to_string(),
                })?;

        let delta_rx = handle.subscribe_deltas().await?;

        Ok(delta_rx)
    }

    async fn get_session_state(&mut self, session_id: SessionId) -> Result<AppState, RuntimeError> {
        if !self.sessions.contains_key(&session_id) {
            self.resume_session(session_id).await?;
        }

        let handle =
            self.sessions
                .get(&session_id)
                .ok_or_else(|| RuntimeError::SessionNotFound {
                    session_id: session_id.to_string(),
                })?;

        let state = handle.get_state().await?;

        Ok(state)
    }

    async fn shutdown_all(&mut self) {
        for (session_id, handle) in self.sessions.drain() {
            handle.shutdown();
            tracing::debug!(session_id = %session_id, "Shutting down session");
        }
    }
}

#[derive(Clone)]
pub struct RuntimeHandle {
    tx: mpsc::Sender<SupervisorCmd>,
}

impl RuntimeHandle {
    pub async fn create_session(&self, config: SessionConfig) -> Result<SessionId, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::CreateSession {
                config: Box::new(config),
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn resume_session(&self, session_id: SessionId) -> Result<(), RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::ResumeSession {
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn suspend_session(&self, session_id: SessionId) -> Result<(), RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::SuspendSession {
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn delete_session(&self, session_id: SessionId) -> Result<(), RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::DeleteSession {
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn dispatch_action(
        &self,
        session_id: SessionId,
        action: Action,
    ) -> Result<(), RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::DispatchAction {
                session_id,
                action: Box::new(action),
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn subscribe_events(
        &self,
        session_id: SessionId,
    ) -> Result<SessionEventSubscription, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::SubscribeEvents {
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn subscribe_deltas(
        &self,
        session_id: SessionId,
    ) -> Result<broadcast::Receiver<StreamDelta>, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::SubscribeDeltas {
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn load_events_after(
        &self,
        session_id: SessionId,
        after_seq: u64,
    ) -> Result<Vec<(u64, SessionEvent)>, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::LoadEventsAfter {
                session_id,
                after_seq,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn get_session_state(&self, session_id: SessionId) -> Result<AppState, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::GetSessionState {
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn is_session_active(&self, session_id: SessionId) -> Result<bool, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::IsSessionActive {
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)
    }

    pub async fn list_active_sessions(&self) -> Result<Vec<SessionId>, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::ListActiveSessions { reply: reply_tx })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)
    }

    pub async fn submit_user_input(
        &self,
        session_id: SessionId,
        content: Vec<UserContent>,
        model: ModelId,
    ) -> Result<(OpId, MessageId), RuntimeError> {
        let has_text = content
            .iter()
            .any(|item| matches!(item, UserContent::Text { text } if !text.trim().is_empty()));
        let has_non_text = content
            .iter()
            .any(|item| !matches!(item, UserContent::Text { .. }));
        if !has_text && !has_non_text {
            return Err(RuntimeError::InvalidInput {
                message: "Input text cannot be empty".to_string(),
            });
        }

        let op_id = OpId::new();
        let message_id = MessageId::new();
        let timestamp = current_timestamp();

        let action = Action::UserInput {
            session_id,
            content,
            op_id,
            message_id: message_id.clone(),
            model,
            timestamp,
        };

        self.dispatch_action(session_id, action).await?;

        Ok((op_id, message_id))
    }

    pub async fn submit_tool_approval(
        &self,
        session_id: SessionId,
        request_id: RequestId,
        approved: bool,
        remember: Option<crate::app::domain::action::ApprovalMemory>,
    ) -> Result<(), RuntimeError> {
        use crate::app::domain::action::ApprovalDecision;

        let decision = if approved {
            ApprovalDecision::Approved
        } else {
            ApprovalDecision::Denied
        };

        let action = Action::ToolApprovalDecided {
            session_id,
            request_id,
            decision,
            remember,
        };

        self.dispatch_action(session_id, action).await
    }

    pub async fn switch_primary_agent(
        &self,
        session_id: SessionId,
        agent_id: String,
    ) -> Result<(), RuntimeError> {
        let action = Action::SwitchPrimaryAgent {
            session_id,
            agent_id,
        };
        self.dispatch_action(session_id, action).await
    }

    pub async fn cancel_operation(
        &self,
        session_id: SessionId,
        op_id: Option<OpId>,
    ) -> Result<(), RuntimeError> {
        let action = Action::Cancel { session_id, op_id };
        self.dispatch_action(session_id, action).await
    }

    pub async fn submit_edited_message(
        &self,
        session_id: SessionId,
        original_message_id: String,
        new_content: Vec<UserContent>,
        model: ModelId,
    ) -> Result<OpId, RuntimeError> {
        let op_id = OpId::new();
        let new_message_id = MessageId::new();
        let timestamp = current_timestamp();

        let action = Action::UserEditedMessage {
            session_id,
            message_id: MessageId::from_string(original_message_id),
            new_content,
            op_id,
            new_message_id,
            model,
            timestamp,
        };

        self.dispatch_action(session_id, action).await?;
        Ok(op_id)
    }

    pub async fn submit_dequeue_queued_item(
        &self,
        session_id: SessionId,
    ) -> Result<(), RuntimeError> {
        let state = self.get_session_state(session_id).await?;
        if state.queued_work.is_empty() {
            return Err(RuntimeError::InvalidInput {
                message: "No queued item to remove".to_string(),
            });
        }

        let action = Action::DequeueQueuedItem { session_id };
        self.dispatch_action(session_id, action).await
    }

    pub async fn compact_session(
        &self,
        session_id: SessionId,
        model: ModelId,
    ) -> Result<OpId, RuntimeError> {
        let op_id = OpId::new();

        let action = Action::RequestCompaction {
            session_id,
            op_id,
            model,
        };

        self.dispatch_action(session_id, action).await?;
        Ok(op_id)
    }

    pub async fn execute_bash_command(
        &self,
        session_id: SessionId,
        command: String,
    ) -> Result<OpId, RuntimeError> {
        let op_id = OpId::new();
        let message_id = MessageId::new();
        let timestamp = current_timestamp();

        let action = Action::DirectBashCommand {
            session_id,
            op_id,
            message_id,
            command,
            timestamp,
        };

        self.dispatch_action(session_id, action).await?;
        Ok(op_id)
    }

    pub async fn list_all_sessions(&self) -> Result<Vec<SessionId>, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::ListAllSessions { reply: reply_tx })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub async fn session_exists(&self, session_id: SessionId) -> Result<bool, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::SessionExists {
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RuntimeError::ChannelClosed)?
    }

    pub fn shutdown(&self) {
        let _ = self.tx.try_send(SupervisorCmd::Shutdown);
    }
}

pub struct RuntimeService {
    pub handle: RuntimeHandle,
    task: JoinHandle<()>,
}

impl RuntimeService {
    pub fn spawn(
        event_store: Arc<dyn EventStore>,
        api_client: Arc<ApiClient>,
        tool_executor: Arc<ToolExecutor>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(64);

        let supervisor = RuntimeSupervisor::new(event_store, api_client, tool_executor);
        let task = tokio::spawn(supervisor.run(rx));

        let handle = RuntimeHandle { tx };

        Self { handle, task }
    }

    pub fn handle(&self) -> RuntimeHandle {
        self.handle.clone()
    }

    pub async fn shutdown(self) {
        self.handle.shutdown();
        let _ = self.task.await;
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::domain::session::event_store::InMemoryEventStore;
    use crate::app::validation::ValidatorRegistry;
    use crate::tools::BackendRegistry;

    async fn create_test_deps() -> (Arc<dyn EventStore>, Arc<ApiClient>, Arc<ToolExecutor>) {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(crate::model_registry::ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider().unwrap(),
            provider_registry,
            model_registry,
        ));

        let tool_executor = Arc::new(ToolExecutor::with_components(
            Arc::new(BackendRegistry::new()),
            Arc::new(ValidatorRegistry::new()),
        ));

        (event_store, api_client, tool_executor)
    }

    fn test_session_config() -> SessionConfig {
        SessionConfig {
            default_model: crate::config::model::builtin::claude_sonnet_4_5(),
            workspace: crate::session::state::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap(),
            },
            workspace_ref: None,
            workspace_id: None,
            repo_ref: None,
            parent_session_id: None,
            workspace_name: None,
            tool_config: crate::session::state::SessionToolConfig::default(),
            system_prompt: None,
            primary_agent_id: None,
            policy_overrides: crate::session::state::SessionPolicyOverrides::empty(),
            title: None,
            metadata: std::collections::HashMap::new(),
            auto_compaction: crate::session::state::AutoCompactionConfig::default(),
        }
    }

    #[tokio::test]
    async fn test_create_session() {
        let (event_store, api_client, tool_executor) = create_test_deps().await;
        let service = RuntimeService::spawn(event_store, api_client, tool_executor);

        let session_id = service
            .handle
            .create_session(test_session_config())
            .await
            .unwrap();

        assert!(service.handle.is_session_active(session_id).await.unwrap());

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_suspend_and_resume_session() {
        let (event_store, api_client, tool_executor) = create_test_deps().await;
        let service = RuntimeService::spawn(event_store, api_client, tool_executor);

        let session_id = service
            .handle
            .create_session(test_session_config())
            .await
            .unwrap();

        service.handle.suspend_session(session_id).await.unwrap();
        assert!(!service.handle.is_session_active(session_id).await.unwrap());

        service.handle.resume_session(session_id).await.unwrap();
        assert!(service.handle.is_session_active(session_id).await.unwrap());

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_delete_session() {
        let (event_store, api_client, tool_executor) = create_test_deps().await;
        let service = RuntimeService::spawn(event_store, api_client, tool_executor);

        let session_id = service
            .handle
            .create_session(test_session_config())
            .await
            .unwrap();

        service.handle.delete_session(session_id).await.unwrap();
        assert!(!service.handle.is_session_active(session_id).await.unwrap());

        let result = service.handle.resume_session(session_id).await;
        assert!(matches!(result, Err(RuntimeError::SessionNotFound { .. })));

        service.shutdown().await;
    }

    #[tokio::test]
    async fn test_subscribe_events() {
        let (event_store, api_client, tool_executor) = create_test_deps().await;
        let service = RuntimeService::spawn(event_store, api_client, tool_executor);

        let session_id = service
            .handle
            .create_session(test_session_config())
            .await
            .unwrap();

        let _subscription = service.handle.subscribe_events(session_id).await.unwrap();

        service.shutdown().await;
    }
}
