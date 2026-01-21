use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::domain::action::{Action, McpServerState};
use crate::app::domain::delta::StreamDelta;
use crate::app::domain::effect::{Effect, McpServerConfig};
use crate::app::domain::event::SessionEvent;
use crate::app::domain::reduce::reduce;
use crate::app::domain::session::{EventStore, EventStoreError};
use crate::app::domain::state::AppState;
use crate::app::domain::types::{MessageId, OpId, SessionId};
use crate::tools::{McpBackend, SessionMcpBackends, ToolBackend, ToolExecutor};

use super::interpreter::{DeltaStreamContext, EffectInterpreter};
use super::subscription::{SessionEventEnvelope, SessionEventSubscription, UnsubscribeSignal};

const EVENT_BROADCAST_CAPACITY: usize = 256;
const DELTA_BROADCAST_CAPACITY: usize = 1024;

pub(crate) enum SessionCmd {
    Dispatch {
        action: Action,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    Subscribe {
        reply: oneshot::Sender<SessionEventSubscription>,
    },
    SubscribeDeltas {
        reply: oneshot::Sender<broadcast::Receiver<StreamDelta>>,
    },
    GetState {
        reply: oneshot::Sender<AppState>,
    },
    Suspend {
        reply: oneshot::Sender<()>,
    },
    Shutdown,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Event store error: {0}")]
    EventStore(#[from] EventStoreError),

    #[error("Session shutting down")]
    ShuttingDown,

    #[error("Channel closed")]
    ChannelClosed,
}

pub(crate) struct SessionActorHandle {
    pub cmd_tx: mpsc::Sender<SessionCmd>,
}

impl SessionActorHandle {
    pub async fn dispatch(&self, action: Action) -> Result<(), SessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(SessionCmd::Dispatch {
                action,
                reply: reply_tx,
            })
            .await
            .map_err(|_| SessionError::ChannelClosed)?;
        reply_rx.await.map_err(|_| SessionError::ChannelClosed)?
    }

    pub async fn subscribe(&self) -> Result<SessionEventSubscription, SessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(SessionCmd::Subscribe { reply: reply_tx })
            .await
            .map_err(|_| SessionError::ChannelClosed)?;
        reply_rx.await.map_err(|_| SessionError::ChannelClosed)
    }

    pub async fn get_state(&self) -> Result<AppState, SessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(SessionCmd::GetState { reply: reply_tx })
            .await
            .map_err(|_| SessionError::ChannelClosed)?;
        reply_rx.await.map_err(|_| SessionError::ChannelClosed)
    }

    pub async fn suspend(&self) -> Result<(), SessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(SessionCmd::Suspend { reply: reply_tx })
            .await
            .map_err(|_| SessionError::ChannelClosed)?;
        reply_rx.await.map_err(|_| SessionError::ChannelClosed)
    }

    pub async fn subscribe_deltas(&self) -> Result<broadcast::Receiver<StreamDelta>, SessionError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(SessionCmd::SubscribeDeltas { reply: reply_tx })
            .await
            .map_err(|_| SessionError::ChannelClosed)?;
        reply_rx.await.map_err(|_| SessionError::ChannelClosed)
    }

    pub fn shutdown(&self) {
        let _ = self.cmd_tx.try_send(SessionCmd::Shutdown);
    }
}

struct SessionActor {
    session_id: SessionId,
    state: AppState,
    event_store: Arc<dyn EventStore>,
    interpreter: EffectInterpreter,
    tool_executor: Arc<ToolExecutor>,
    active_operations: HashMap<OpId, CancellationToken>,
    event_broadcast: broadcast::Sender<SessionEventEnvelope>,
    delta_broadcast: broadcast::Sender<StreamDelta>,
    subscriber_count: usize,
    unsubscribe_rx: mpsc::UnboundedReceiver<UnsubscribeSignal>,
    unsubscribe_tx: mpsc::UnboundedSender<UnsubscribeSignal>,
    internal_action_tx: mpsc::Sender<Action>,
    internal_action_rx: mpsc::Receiver<Action>,
    session_mcp_backends: Arc<SessionMcpBackends>,
}

impl SessionActor {
    fn new(
        session_id: SessionId,
        state: AppState,
        event_store: Arc<dyn EventStore>,
        api_client: Arc<ApiClient>,
        tool_executor: Arc<ToolExecutor>,
    ) -> Self {
        let (event_broadcast, _) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
        let (delta_broadcast, _) = broadcast::channel(DELTA_BROADCAST_CAPACITY);
        let (unsubscribe_tx, unsubscribe_rx) = mpsc::unbounded_channel();
        let (internal_action_tx, internal_action_rx) = mpsc::channel(64);
        let session_mcp_backends = Arc::new(SessionMcpBackends::new());
        let interpreter = EffectInterpreter::new(api_client, tool_executor.clone())
            .with_session(session_id)
            .with_session_backends(session_mcp_backends.clone());

        Self {
            session_id,
            state,
            event_store,
            interpreter,
            tool_executor,
            active_operations: HashMap::new(),
            event_broadcast,
            delta_broadcast,
            subscriber_count: 0,
            unsubscribe_rx,
            unsubscribe_tx,
            internal_action_tx,
            internal_action_rx,
            session_mcp_backends,
        }
    }

    async fn run(mut self, mut cmd_rx: mpsc::Receiver<SessionCmd>) {
        self.load_initial_tool_schemas().await;
        self.initialize_mcp_connections().await;

        loop {
            tokio::select! {
                biased;

                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        SessionCmd::Dispatch { action, reply } => {
                            let result = self.handle_action(action).await;
                            let _ = reply.send(result);
                        }
                        SessionCmd::Subscribe { reply } => {
                            let subscription = self.create_subscription();
                            let _ = reply.send(subscription);
                        }
                        SessionCmd::SubscribeDeltas { reply } => {
                            let rx = self.delta_broadcast.subscribe();
                            let _ = reply.send(rx);
                        }
                        SessionCmd::GetState { reply } => {
                            let _ = reply.send(self.state.clone());
                        }
                        SessionCmd::Suspend { reply } => {
                            self.cleanup_mcp_backends().await;
                            let _ = reply.send(());
                            break;
                        }
                        SessionCmd::Shutdown => {
                            self.cancel_all_operations();
                            self.cleanup_mcp_backends().await;
                            break;
                        }
                    }
                }

                Some(action) = self.internal_action_rx.recv() => {
                    if let Err(e) = self.handle_action(action).await {
                        tracing::error!(
                            session_id = %self.session_id,
                            error = %e,
                            "Failed to handle internal action"
                        );
                    }
                }

                Some(UnsubscribeSignal) = self.unsubscribe_rx.recv() => {
                    self.subscriber_count = self.subscriber_count.saturating_sub(1);
                    tracing::debug!(
                        session_id = %self.session_id,
                        subscriber_count = self.subscriber_count,
                        "Subscriber disconnected"
                    );
                }

                else => break,
            }
        }

        tracing::debug!(session_id = %self.session_id, "Session actor stopped");
    }

    async fn load_initial_tool_schemas(&mut self) {
        let schemas = self.tool_executor.get_tool_schemas().await;
        let schemas = match &self.state.session_config {
            Some(config) => config.filter_tools_by_visibility(schemas),
            None => schemas,
        };

        if let Err(e) = self
            .handle_action(Action::ToolSchemasAvailable {
                session_id: self.session_id,
                tools: schemas,
            })
            .await
        {
            tracing::error!(
                session_id = %self.session_id,
                error = %e,
                "Failed to load initial tool schemas"
            );
        }
    }

    async fn initialize_mcp_connections(&mut self) {
        use crate::app::domain::effect::McpServerConfig;
        use crate::session::state::BackendConfig;

        let effects: Vec<_> = self
            .state
            .session_config
            .as_ref()
            .map(|config| {
                config
                    .tool_config
                    .backends
                    .iter()
                    .map(|backend_config| {
                        let BackendConfig::Mcp {
                            server_name,
                            transport,
                            tool_filter,
                        } = backend_config;

                        Effect::ConnectMcpServer {
                            session_id: self.session_id,
                            config: McpServerConfig {
                                server_name: server_name.clone(),
                                transport: transport.clone(),
                                tool_filter: tool_filter.clone(),
                            },
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        for effect in effects {
            if let Err(e) = self.handle_effect(effect).await {
                tracing::error!(
                    session_id = %self.session_id,
                    error = %e,
                    "Failed to initiate MCP connection"
                );
            }
        }
    }

    async fn handle_action(&mut self, action: Action) -> Result<(), SessionError> {
        let effects = reduce(&mut self.state, action);

        for effect in effects {
            self.handle_effect(effect).await?;
        }

        Ok(())
    }

    async fn handle_effect(&mut self, effect: Effect) -> Result<(), SessionError> {
        match effect {
            Effect::EmitEvent { event, .. } => {
                let seq = self.event_store.append(self.session_id, &event).await?;

                let envelope = SessionEventEnvelope { seq, event };
                let _ = self.event_broadcast.send(envelope);

                Ok(())
            }

            Effect::CallModel {
                op_id,
                model,
                messages,
                system_context,
                tools,
                ..
            } => {
                let cancel_token = self.active_operations.entry(op_id).or_default().clone();

                let interpreter = self.interpreter.clone();
                let action_tx = self.internal_action_tx.clone();
                let session_id = self.session_id;
                let delta_broadcast = self.delta_broadcast.clone();
                let message_id = MessageId::new();

                tokio::spawn(async move {
                    let (delta_tx, mut delta_rx) = mpsc::channel::<StreamDelta>(64);
                    let delta_stream = Some(DeltaStreamContext::new(
                        delta_tx,
                        (op_id, message_id.clone()),
                    ));

                    let delta_forward_task = {
                        let delta_broadcast = delta_broadcast.clone();
                        tokio::spawn(async move {
                            while let Some(delta) = delta_rx.recv().await {
                                let _ = delta_broadcast.send(delta);
                            }
                        })
                    };

                    let result = interpreter
                        .call_model_with_deltas(
                            model,
                            messages,
                            system_context,
                            tools,
                            cancel_token,
                            delta_stream,
                        )
                        .await;

                    if let Err(e) = delta_forward_task.await {
                        tracing::debug!(
                            session_id = %session_id,
                            error = %e,
                            "Delta forward task ended unexpectedly"
                        );
                    }

                    let action = match result {
                        Ok(content) => Action::ModelResponseComplete {
                            session_id,
                            op_id,
                            message_id,
                            content,
                            timestamp: current_timestamp(),
                        },
                        Err(e) => Action::ModelResponseError {
                            session_id,
                            op_id,
                            error: e.clone(),
                        },
                    };

                    let _ = action_tx.send(action).await;
                });

                Ok(())
            }

            Effect::ExecuteTool {
                op_id, tool_call, ..
            } => {
                let cancel_token = self.active_operations.entry(op_id).or_default().clone();

                let interpreter = self.interpreter.clone();
                let action_tx = self.internal_action_tx.clone();
                let session_id = self.session_id;
                let tool_call_id =
                    crate::app::domain::types::ToolCallId::from_string(&tool_call.id);
                let tool_name = tool_call.name.clone();
                let tool_parameters = tool_call.parameters.clone();

                let start_action = Action::ToolExecutionStarted {
                    session_id,
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    tool_parameters,
                };
                let _ = action_tx.send(start_action).await;

                tokio::spawn(async move {
                    let result = interpreter.execute_tool(tool_call, cancel_token).await;

                    let action = Action::ToolResult {
                        session_id,
                        tool_call_id,
                        tool_name,
                        result,
                    };

                    let _ = action_tx.send(action).await;
                });

                Ok(())
            }

            Effect::RequestUserApproval {
                request_id,
                tool_call,
                ..
            } => {
                let event = SessionEvent::ApprovalRequested {
                    request_id,
                    tool_call,
                };
                let seq = self.event_store.append(self.session_id, &event).await?;
                let envelope = SessionEventEnvelope { seq, event };
                let _ = self.event_broadcast.send(envelope);
                Ok(())
            }

            Effect::CancelOperation { op_id, .. } => {
                if let Some(token) = self.active_operations.remove(&op_id) {
                    token.cancel();
                }
                Ok(())
            }

            Effect::ListWorkspaceFiles { .. } => Ok(()),

            Effect::ConnectMcpServer { config, .. } => {
                self.handle_connect_mcp_server(config).await;
                Ok(())
            }

            Effect::DisconnectMcpServer { server_name, .. } => {
                self.handle_disconnect_mcp_server(server_name).await;
                Ok(())
            }

            Effect::RequestCompaction {
                session_id,
                op_id,
                model,
            } => {
                let cancel_token = self.active_operations.entry(op_id).or_default().clone();

                let interpreter = self.interpreter.clone();
                let action_tx = self.internal_action_tx.clone();

                let messages: Vec<crate::app::conversation::Message> = self
                    .state
                    .message_graph
                    .get_thread_messages()
                    .into_iter()
                    .cloned()
                    .collect();
                let compacted_head = self
                    .state
                    .message_graph
                    .active_message_id
                    .clone()
                    .map(MessageId::from)
                    .unwrap_or_default();
                let previous_active = self
                    .state
                    .message_graph
                    .active_message_id
                    .clone()
                    .map(MessageId::from);
                tokio::spawn(async move {
                    let summary_prompt = build_compaction_prompt(&messages);

                    let result = interpreter
                        .call_model(
                            model.clone(),
                            vec![summary_prompt],
                            None,
                            vec![],
                            cancel_token,
                        )
                        .await;

                    let action = match result {
                        Ok(content) => {
                            let summary_text = content
                                .iter()
                                .filter_map(|c| match c {
                                    crate::app::conversation::AssistantContent::Text { text } => {
                                        Some(text.as_str())
                                    }
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n");

                            Action::CompactionComplete {
                                session_id,
                                op_id,
                                compaction_id: crate::app::domain::types::CompactionId::new(),
                                summary_message_id: MessageId::new(),
                                summary: summary_text,
                                compacted_head_message_id: compacted_head,
                                previous_active_message_id: previous_active,
                                model: model.id.clone(),
                                timestamp: current_timestamp(),
                            }
                        }
                        Err(e) => Action::CompactionFailed {
                            session_id,
                            op_id,
                            error: e,
                        },
                    };

                    let _ = action_tx.send(action).await;
                });

                Ok(())
            }
        }
    }

    async fn handle_connect_mcp_server(&self, config: McpServerConfig) {
        let server_name = config.server_name.clone();
        let session_id = self.session_id;
        let action_tx = self.internal_action_tx.clone();
        let session_backends = self.session_mcp_backends.clone();
        let generation = session_backends.next_generation(&server_name).await;

        action_tx
            .send(Action::McpServerStateChanged {
                session_id,
                server_name: server_name.clone(),
                state: McpServerState::Connecting,
            })
            .await
            .ok();

        tokio::spawn(async move {
            let result = McpBackend::new(
                config.server_name.clone(),
                config.transport,
                config.tool_filter,
            )
            .await;

            if !session_backends
                .is_current_generation(&server_name, generation)
                .await
            {
                return;
            }

            match result {
                Ok(backend) => {
                    let tools = backend.get_tool_schemas().await;
                    let backend = Arc::new(backend);
                    session_backends
                        .register(server_name.clone(), backend)
                        .await;

                    if !session_backends
                        .is_current_generation(&server_name, generation)
                        .await
                    {
                        let _ = session_backends.unregister(&server_name).await;
                        return;
                    }

                    action_tx
                        .send(Action::McpServerStateChanged {
                            session_id,
                            server_name,
                            state: McpServerState::Connected { tools },
                        })
                        .await
                        .ok();
                }
                Err(e) => {
                    tracing::error!(
                        session_id = %session_id,
                        server_name = %server_name,
                        error = %e,
                        "Failed to connect to MCP server"
                    );

                    if session_backends
                        .is_current_generation(&server_name, generation)
                        .await
                    {
                        action_tx
                            .send(Action::McpServerStateChanged {
                                session_id,
                                server_name,
                                state: McpServerState::Failed {
                                    error: e.to_string(),
                                },
                            })
                            .await
                            .ok();
                    }
                }
            }
        });
    }

    async fn handle_disconnect_mcp_server(&self, server_name: String) {
        let session_id = self.session_id;

        self.session_mcp_backends
            .next_generation(&server_name)
            .await;
        let _ = self.session_mcp_backends.unregister(&server_name).await;

        self.internal_action_tx
            .send(Action::McpServerStateChanged {
                session_id,
                server_name,
                state: McpServerState::Disconnected { error: None },
            })
            .await
            .ok();
    }

    fn create_subscription(&mut self) -> SessionEventSubscription {
        self.subscriber_count += 1;
        tracing::debug!(
            session_id = %self.session_id,
            subscriber_count = self.subscriber_count,
            "New subscriber"
        );

        let rx = self.event_broadcast.subscribe();
        SessionEventSubscription::new(self.session_id, rx, self.unsubscribe_tx.clone())
    }

    fn cancel_all_operations(&mut self) {
        for (_, token) in self.active_operations.drain() {
            token.cancel();
        }
    }

    async fn cleanup_mcp_backends(&self) {
        self.session_mcp_backends.clear().await;
    }
}

pub(crate) fn spawn_session_actor(
    session_id: SessionId,
    state: AppState,
    event_store: Arc<dyn EventStore>,
    api_client: Arc<ApiClient>,
    tool_executor: Arc<ToolExecutor>,
) -> SessionActorHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(32);

    let actor = SessionActor::new(session_id, state, event_store, api_client, tool_executor);

    tokio::spawn(actor.run(cmd_rx));

    SessionActorHandle { cmd_tx }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn build_compaction_prompt(
    messages: &[crate::app::conversation::Message],
) -> crate::app::conversation::Message {
    use crate::app::conversation::{Message, MessageData, UserContent};

    let conversation_text = messages
        .iter()
        .map(format_message_for_summary)
        .collect::<Vec<_>>()
        .join("\n\n");

    let prompt = format!(
        "Summarize this conversation concisely, preserving:\n\
         1. Key decisions and conclusions\n\
         2. Important context established\n\
         3. Current task state\n\
         4. Critical information needed to continue\n\n\
         Conversation:\n{conversation_text}\n\n\
         Summary:"
    );

    Message {
        data: MessageData::User {
            content: vec![UserContent::Text { text: prompt }],
        },
        id: uuid::Uuid::new_v4().to_string(),
        parent_message_id: None,
        timestamp: current_timestamp(),
    }
}

fn format_message_for_summary(message: &crate::app::conversation::Message) -> String {
    use crate::app::conversation::{AssistantContent, MessageData, UserContent};

    match &message.data {
        MessageData::User { content } => {
            let text = content
                .iter()
                .filter_map(|c| match c {
                    UserContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("User: {text}")
        }
        MessageData::Assistant { content } => {
            let text = content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("Assistant: {text}")
        }
        MessageData::Tool { .. } => String::new(),
    }
}
