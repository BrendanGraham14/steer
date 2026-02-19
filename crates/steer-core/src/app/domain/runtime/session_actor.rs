use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::api::provider::CompletionResponse;
use crate::app::domain::action::{
    Action, McpServerState, ModelCallError, SchemaSource, SessionTitleGenerationError,
};
use crate::app::domain::delta::StreamDelta;
use crate::app::domain::effect::{Effect, McpServerConfig};
use crate::app::domain::event::SessionEvent;
use crate::app::domain::reduce::{InvalidActionKind, ReduceError, reduce};
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
        action: Box<Action>,
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

    #[error("Invalid input: {message}")]
    InvalidInput {
        message: String,
        kind: InvalidActionKind,
    },

    #[error("Reduce error: {message}")]
    ReduceError { message: String },

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
                action: Box::new(action),
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
                            let result = self.handle_action(*action).await;
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
        let effects = reduce(&mut self.state, action).map_err(|err| match err {
            ReduceError::InvalidAction { message, kind } => {
                SessionError::InvalidInput { message, kind }
            }
            ReduceError::Invariant { message } => SessionError::ReduceError { message },
        })?;

        for effect in effects {
            self.handle_effect(effect).await?;
        }

        Ok(())
    }

    async fn handle_effect(&mut self, effect: Effect) -> Result<(), SessionError> {
        match effect {
            Effect::EmitEvent { event, .. } => {
                let seq = match self.event_store.append(self.session_id, &event).await {
                    Ok(seq) => seq,
                    Err(e) => {
                        tracing::error!(
                            target: "core.event_store",
                            session_id = %self.session_id,
                            event = ?event,
                            error = %e,
                            "Failed to append session event"
                        );
                        return Err(SessionError::EventStore(e));
                    }
                };

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
                let context_window_tokens = self.interpreter.model_context_window_tokens(&model);
                let configured_max_output_tokens = self.interpreter.model_max_output_tokens(&model);
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
                        Ok(CompletionResponse { content, usage }) => {
                            Action::ModelResponseComplete {
                                session_id,
                                op_id,
                                message_id,
                                content,
                                usage,
                                context_window_tokens,
                                configured_max_output_tokens,
                                timestamp: current_timestamp(),
                            }
                        }
                        Err(error) => Action::ModelResponseError {
                            session_id,
                            op_id,
                            error: error.to_string(),
                        },
                    };

                    let _ = action_tx.send(action).await;
                });

                Ok(())
            }

            Effect::GenerateSessionTitle {
                op_id,
                model,
                user_prompt,
                ..
            } => {
                let interpreter = self.interpreter.clone();
                let action_tx = self.internal_action_tx.clone();
                let session_id = self.session_id;
                let system_context = self.state.cached_system_context.clone();
                let cancel_token = self.active_operations.entry(op_id).or_default().clone();

                tokio::spawn(async move {
                    let title_request = vec![crate::app::conversation::Message {
                        data: crate::app::conversation::MessageData::User {
                            content: vec![crate::app::conversation::UserContent::Text {
                                text: format!(
                                    "Generate a concise session title (max 8 words). Return only the title.\n\nUser request:\n{}",
                                    user_prompt
                                ),
                            }],
                        },
                        timestamp: current_timestamp(),
                        id: uuid::Uuid::new_v4().to_string(),
                        parent_message_id: None,
                    }];

                    let action = match interpreter
                        .call_model(model, title_request, system_context, vec![], cancel_token)
                        .await
                    {
                        Ok(response) => {
                            let title = response
                                .content
                                .iter()
                                .find_map(|item| match item {
                                    crate::app::conversation::AssistantContent::Text { text } => {
                                        Some(text.trim().to_string())
                                    }
                                    _ => None,
                                })
                                .filter(|title| !title.is_empty())
                                .unwrap_or_else(|| "Untitled Session".to_string());

                            Action::SessionTitleGenerated { session_id, title }
                        }
                        Err(error) => Action::SessionTitleGenerationFailed {
                            session_id,
                            error: SessionTitleGenerationError::from(error),
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
                let invoking_model = self.state.operation_models.get(&op_id).cloned();

                let start_action = Action::ToolExecutionStarted {
                    session_id,
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    tool_parameters,
                };
                let _ = action_tx.send(start_action).await;

                tokio::spawn(async move {
                    let result = interpreter
                        .execute_tool(tool_call, invoking_model, cancel_token)
                        .await;

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
                let system_context = self.state.cached_system_context.clone();
                tokio::spawn(async move {
                    let mut compaction_messages = messages;
                    let compaction_prompt = build_compaction_message();
                    let mut dropped_tool_results = 0usize;

                    let result = loop {
                        let request_messages = compaction_messages
                            .iter()
                            .cloned()
                            .chain(std::iter::once(compaction_prompt.clone()))
                            .collect();

                        match interpreter
                            .call_model(
                                model.clone(),
                                request_messages,
                                system_context.clone(),
                                vec![],
                                cancel_token.clone(),
                            )
                            .await
                        {
                            Ok(response) => break Ok(response),
                            Err(error) if is_context_window_exceeded_error(&error) => {
                                let dropped = drop_earlier_tool_results(&mut compaction_messages);
                                if dropped == 0 {
                                    break Err(error);
                                }

                                dropped_tool_results += dropped;
                                tracing::warn!(
                                    session_id = %session_id,
                                    op_id = %op_id,
                                    dropped,
                                    dropped_tool_results,
                                    "Compaction context window exceeded; retrying after dropping earlier tool results"
                                );
                            }
                            Err(error) => break Err(error),
                        }
                    };

                    let action = match result {
                        Ok(response) => {
                            let summary_text = response
                                .content
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
                        Err(error) => Action::CompactionFailed {
                            session_id,
                            op_id,
                            error: if dropped_tool_results > 0 {
                                format!(
                                    "{error} (compaction retried after dropping {dropped_tool_results} earlier tool results)"
                                )
                            } else {
                                error.to_string()
                            },
                        },
                    };

                    let _ = action_tx.send(action).await;
                });

                Ok(())
            }

            Effect::ReloadToolSchemas { session_id } => {
                let resolver =
                    self.session_mcp_backends.as_ref() as &dyn crate::tools::BackendResolver;
                let schemas = self
                    .tool_executor
                    .get_tool_schemas_with_resolver(Some(resolver))
                    .await;
                let schemas = match &self.state.session_config {
                    Some(config) => config.filter_tools_by_visibility(schemas),
                    None => schemas,
                };

                if let Err(err) = self
                    .internal_action_tx
                    .send(Action::ToolSchemasUpdated {
                        session_id,
                        schemas,
                        source: SchemaSource::Backend {
                            backend_name: "tool_executor".to_string(),
                        },
                    })
                    .await
                {
                    tracing::error!(
                        session_id = %self.session_id,
                        error = %err,
                        "Failed to dispatch tool schema reload"
                    );
                }

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

fn is_context_window_exceeded_error(error: &ModelCallError) -> bool {
    let normalized = error.to_string().to_ascii_lowercase();

    let has_explicit_phrase = [
        "context length",
        "context window",
        "maximum context",
        "max context",
        "context_length_exceeded",
        "too many tokens",
        "token limit",
        "prompt is too long",
        "input is too long",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase));

    has_explicit_phrase
        || (normalized.contains("context") && normalized.contains("exceed"))
        || (normalized.contains("token")
            && (normalized.contains("exceed")
                || normalized.contains("too many")
                || normalized.contains("limit")))
}

fn drop_earlier_tool_results(messages: &mut Vec<crate::app::conversation::Message>) -> usize {
    let dropped_stale_reads = drop_stale_read_file_results(messages);
    if dropped_stale_reads > 0 {
        return dropped_stale_reads;
    }

    drop_oldest_tool_results(messages)
}

#[derive(Debug)]
struct ToolCallMetadata {
    name: String,
    file_path: Option<String>,
}

fn drop_stale_read_file_results(messages: &mut Vec<crate::app::conversation::Message>) -> usize {
    let tool_call_metadata = collect_tool_call_metadata(messages);

    let mut read_results_by_path: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    let mut latest_edit_index_by_path: HashMap<String, usize> = HashMap::new();

    for (index, message) in messages.iter().enumerate() {
        let crate::app::conversation::MessageData::Tool {
            tool_use_id,
            result,
        } = &message.data
        else {
            continue;
        };

        let Some(metadata) = tool_call_metadata.get(tool_use_id) else {
            continue;
        };

        let Some(file_path) = metadata.file_path.as_ref() else {
            continue;
        };

        match metadata.name.as_str() {
            steer_tools::tools::VIEW_TOOL_NAME
                if matches!(result, crate::app::conversation::ToolResult::FileContent(_)) =>
            {
                read_results_by_path
                    .entry(file_path.clone())
                    .or_default()
                    .push((index, tool_use_id.clone()));
            }
            steer_tools::tools::EDIT_TOOL_NAME
            | steer_tools::tools::MULTI_EDIT_TOOL_NAME
            | steer_tools::tools::REPLACE_TOOL_NAME
                if matches!(result, crate::app::conversation::ToolResult::Edit(_)) =>
            {
                latest_edit_index_by_path.insert(file_path.clone(), index);
            }
            _ => {}
        }
    }

    let mut ids_to_drop = HashSet::new();

    for (file_path, reads) in &read_results_by_path {
        if let Some((_, newest_id)) = reads.iter().max_by_key(|(index, _)| *index) {
            for (_, id) in reads {
                if id != newest_id {
                    ids_to_drop.insert(id.clone());
                }
            }
        }

        if let Some(latest_edit_index) = latest_edit_index_by_path.get(file_path) {
            for (index, id) in reads {
                if index < latest_edit_index {
                    ids_to_drop.insert(id.clone());
                }
            }
        }
    }

    if ids_to_drop.is_empty() {
        return 0;
    }

    drop_tool_results_with_matching_tool_calls(messages, &ids_to_drop)
}

fn drop_oldest_tool_results(messages: &mut Vec<crate::app::conversation::Message>) -> usize {
    let tool_result_ids: Vec<String> = messages
        .iter()
        .filter_map(|message| {
            if let crate::app::conversation::MessageData::Tool { tool_use_id, .. } = &message.data {
                Some(tool_use_id.clone())
            } else {
                None
            }
        })
        .collect();

    if tool_result_ids.is_empty() {
        return 0;
    }

    let target_drop_count = (tool_result_ids.len() / 2).max(1);
    let ids_to_drop: HashSet<String> = tool_result_ids
        .into_iter()
        .take(target_drop_count)
        .collect();

    drop_tool_results_with_matching_tool_calls(messages, &ids_to_drop)
}

fn collect_tool_call_metadata(
    messages: &[crate::app::conversation::Message],
) -> HashMap<String, ToolCallMetadata> {
    let mut metadata = HashMap::new();

    for message in messages {
        let crate::app::conversation::MessageData::Assistant { content } = &message.data else {
            continue;
        };

        for block in content {
            let crate::app::conversation::AssistantContent::ToolCall { tool_call, .. } = block
            else {
                continue;
            };

            let file_path = match tool_call.name.as_str() {
                steer_tools::tools::VIEW_TOOL_NAME
                | steer_tools::tools::EDIT_TOOL_NAME
                | steer_tools::tools::MULTI_EDIT_TOOL_NAME
                | steer_tools::tools::REPLACE_TOOL_NAME => tool_call
                    .parameters
                    .as_object()
                    .and_then(|params| params.get("file_path"))
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string()),
                _ => None,
            };

            metadata.insert(
                tool_call.id.clone(),
                ToolCallMetadata {
                    name: tool_call.name.clone(),
                    file_path,
                },
            );
        }
    }

    metadata
}

fn drop_tool_results_with_matching_tool_calls(
    messages: &mut Vec<crate::app::conversation::Message>,
    tool_use_ids_to_drop: &HashSet<String>,
) -> usize {
    if tool_use_ids_to_drop.is_empty() {
        return 0;
    }

    let mut dropped_tool_results = 0usize;
    let mut pruned_messages = Vec::with_capacity(messages.len());

    for mut message in messages.drain(..) {
        match &mut message.data {
            crate::app::conversation::MessageData::Tool { tool_use_id, .. } => {
                if tool_use_ids_to_drop.contains(tool_use_id) {
                    dropped_tool_results += 1;
                } else {
                    pruned_messages.push(message);
                }
            }
            crate::app::conversation::MessageData::Assistant { content } => {
                let original_len = content.len();
                content.retain(|block| {
                    if let crate::app::conversation::AssistantContent::ToolCall {
                        tool_call, ..
                    } = block
                    {
                        !tool_use_ids_to_drop.contains(&tool_call.id)
                    } else {
                        true
                    }
                });

                let removed_tool_call = original_len != content.len();
                if removed_tool_call && !assistant_message_has_request_relevant_content(content) {
                    continue;
                }

                pruned_messages.push(message);
            }
            crate::app::conversation::MessageData::User { .. } => {
                pruned_messages.push(message);
            }
        }
    }

    *messages = pruned_messages;
    dropped_tool_results
}

fn assistant_message_has_request_relevant_content(
    content: &[crate::app::conversation::AssistantContent],
) -> bool {
    content.iter().any(|block| {
        !matches!(
            block,
            crate::app::conversation::AssistantContent::Thought { .. }
        )
    })
}

const COMPACTION_PROMPT: &str = r"
You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.

Include:
    - Current progress and key decisions made
    - Important context, constraints, or user preferences discovered during this session
    - What remains to be done (clear next steps)
    - Any critical data, examples, or references needed to continue

DO NOT include:
    - System context information (repo structure, VCS state, environment details) - the next LLM will have its own system context
    - Tool schemas or capabilities - these are provided separately
    - General project information already in the system prompt

Be concise, structured, and focused on session-specific progress and learnings.";

fn build_compaction_message() -> crate::app::conversation::Message {
    crate::app::conversation::Message {
        data: crate::app::conversation::MessageData::User {
            content: vec![crate::app::conversation::UserContent::Text {
                text: COMPACTION_PROMPT.to_string(),
            }],
        },
        id: uuid::Uuid::new_v4().to_string(),
        parent_message_id: None,
        timestamp: current_timestamp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::ApiError;
    use crate::api::provider::{CompletionResponse, Provider, StreamChunk, TokenUsage};
    use crate::app::SystemContext;
    use crate::app::conversation::{
        AssistantContent, Message, MessageData, ThoughtContent, UserContent,
    };
    use crate::app::domain::action::Action;
    use crate::app::domain::event::{CompactResult, CompactTrigger, SessionEvent};
    use crate::app::domain::session::event_store::InMemoryEventStore;
    use crate::app::domain::state::OperationKind;
    use crate::app::domain::types::{MessageId, OpId, SessionId};
    use crate::app::validation::ValidatorRegistry;
    use crate::auth::ProviderRegistry;
    use crate::config::model::{ModelId, ModelParameters, builtin};
    use crate::config::provider::ProviderId;
    use crate::model_registry::ModelRegistry;
    use crate::session::state::{AutoCompactionConfig, SessionConfig};
    use crate::tools::{BackendRegistry, ToolSystemBuilder};
    use async_trait::async_trait;
    use serde_json::json;
    use steer_tools::ToolCall;
    use steer_tools::ToolSchema;
    use steer_tools::result::{EditResult, ExternalResult, FileContentResult};
    use tempfile::TempDir;
    use tokio::time::{Duration, timeout};
    use tokio_util::sync::CancellationToken;

    #[derive(Clone)]
    struct StubProvider;

    #[derive(Clone)]
    struct StubProviderWithUsage;

    #[derive(Clone)]
    struct BlockingStreamProvider {
        release_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    }

    #[derive(Clone)]
    struct ContextWindowLimitProvider {
        max_tool_messages: usize,
        observed_tool_message_counts: Arc<std::sync::Mutex<Vec<usize>>>,
    }

    #[derive(Clone)]
    struct RepeatedReadThenOverflowProvider {
        file_path: String,
        overflow_char_limit: usize,
    }
    #[async_trait]
    impl Provider for BlockingStreamProvider {
        fn name(&self) -> &'static str {
            "blocking-stream"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: "unused".to_string(),
                }],
                usage: None,
            })
        }

        async fn stream_complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<ModelParameters>,
            token: CancellationToken,
        ) -> Result<crate::api::provider::CompletionStream, ApiError> {
            let receiver = self
                .release_rx
                .lock()
                .await
                .take()
                .expect("release receiver should be available");

            Ok(Box::pin(futures_util::stream::once(async move {
                tokio::select! {
                    _ = token.cancelled() => StreamChunk::MessageComplete(CompletionResponse {
                        content: vec![],
                        usage: None,
                    }),
                    _ = receiver => StreamChunk::MessageComplete(CompletionResponse {
                        content: vec![AssistantContent::Text {
                            text: "late title".to_string(),
                        }],
                        usage: None,
                    }),
                }
            })))
        }
    }

    #[async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &'static str {
            "stub"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: "summary".to_string(),
                }],
                usage: None,
            })
        }
    }

    #[async_trait]
    impl Provider for StubProviderWithUsage {
        fn name(&self) -> &'static str {
            "stub"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: "reply".to_string(),
                }],
                usage: Some(TokenUsage::new(11, 13, 24)),
            })
        }
    }

    #[async_trait]
    impl Provider for ContextWindowLimitProvider {
        fn name(&self) -> &'static str {
            "stub"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            let tool_messages = messages
                .iter()
                .filter(|message| matches!(message.data, MessageData::Tool { .. }))
                .count();

            self.observed_tool_message_counts
                .lock()
                .expect("lock observed tool message counts")
                .push(tool_messages);

            if tool_messages > self.max_tool_messages {
                return Err(ApiError::InvalidRequest {
                    provider: "stub".to_string(),
                    details: "maximum context length exceeded".to_string(),
                });
            }

            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: "summary".to_string(),
                }],
                usage: None,
            })
        }
    }

    #[async_trait]
    impl Provider for RepeatedReadThenOverflowProvider {
        fn name(&self) -> &'static str {
            "stub"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            let total_chars: usize = messages
                .iter()
                .map(|message| message.extract_text().len())
                .sum();
            if total_chars >= self.overflow_char_limit {
                return Err(ApiError::InvalidRequest {
                    provider: "stub".to_string(),
                    details: "context_length_exceeded".to_string(),
                });
            }

            let read_call_count = messages
                .iter()
                .filter_map(|message| {
                    let MessageData::Assistant { content } = &message.data else {
                        return None;
                    };
                    Some(
                        content
                            .iter()
                            .filter(|block| {
                                matches!(
                                    block,
                                    AssistantContent::ToolCall { tool_call, .. }
                                        if tool_call.name == steer_tools::tools::VIEW_TOOL_NAME
                                )
                            })
                            .count(),
                    )
                })
                .sum::<usize>();

            if read_call_count < 5 {
                Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall {
                        tool_call: ToolCall {
                            id: format!("simulated_read_{read_call_count}"),
                            name: steer_tools::tools::VIEW_TOOL_NAME.to_string(),
                            parameters: json!({
                                "file_path": self.file_path,
                                "offset": read_call_count * 2000 + 1,
                                "limit": 2000,
                            }),
                        },
                        thought_signature: None,
                    }],
                    usage: Some(TokenUsage::new(0, 0, 10_000)),
                })
            } else {
                Ok(CompletionResponse {
                    content: vec![AssistantContent::Text {
                        text: "done".to_string(),
                    }],
                    usage: Some(TokenUsage::new(0, 0, 12_000)),
                })
            }
        }
    }

    async fn create_test_deps() -> (Arc<dyn EventStore>, Arc<ApiClient>, Arc<ToolExecutor>) {
        let event_store = Arc::new(InMemoryEventStore::new()) as Arc<dyn EventStore>;
        let model_registry = Arc::new(ModelRegistry::load(&[]).expect("model registry"));
        let provider_registry = Arc::new(ProviderRegistry::load(&[]).expect("provider registry"));
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider().unwrap(),
            provider_registry,
            model_registry.clone(),
        ));

        let workspace =
            crate::workspace::create_workspace(&crate::workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().expect("current dir"),
            })
            .await
            .expect("create workspace");

        let tool_executor = ToolSystemBuilder::new(
            workspace,
            event_store.clone(),
            api_client.clone(),
            model_registry,
        )
        .with_backend_registry(Arc::new(BackendRegistry::new()))
        .with_validators(Arc::new(ValidatorRegistry::new()))
        .build();

        (event_store, api_client, tool_executor)
    }

    fn seed_messages(state: &mut AppState) {
        let first = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "hello".to_string(),
                }],
            },
            id: "u1".to_string(),
            parent_message_id: None,
            timestamp: 1,
        };
        state.message_graph.add_message(first);

        let second = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "hi".to_string(),
                }],
            },
            id: "a1".to_string(),
            parent_message_id: Some("u1".to_string()),
            timestamp: 2,
        };
        state.message_graph.add_message(second);

        let third = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "summarize".to_string(),
                }],
            },
            id: "u2".to_string(),
            parent_message_id: Some("a1".to_string()),
            timestamp: 3,
        };
        state.message_graph.add_message(third);
    }

    fn seed_messages_with_tool_results(state: &mut AppState, tool_message_count: usize) {
        let first = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "hello".to_string(),
                }],
            },
            id: "u1".to_string(),
            parent_message_id: None,
            timestamp: 1,
        };
        state.message_graph.add_message(first);

        let mut parent_message_id = Some("u1".to_string());
        for index in 0..tool_message_count {
            let id = format!("t{}", index + 1);
            let tool_message = Message {
                data: MessageData::Tool {
                    tool_use_id: format!("tool-call-{}", index + 1),
                    result: crate::app::conversation::ToolResult::External(ExternalResult {
                        tool_name: "stub".to_string(),
                        payload: format!("payload-{}", index + 1),
                    }),
                },
                id: id.clone(),
                parent_message_id: parent_message_id.clone(),
                timestamp: (index as u64) + 2,
            };
            state.message_graph.add_message(tool_message);
            parent_message_id = Some(id);
        }

        let tail = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "summarize".to_string(),
                }],
            },
            id: "u2".to_string(),
            parent_message_id,
            timestamp: (tool_message_count as u64) + 2,
        };
        state.message_graph.add_message(tail);
    }

    fn auto_compact_test_state(session_id: SessionId) -> AppState {
        let mut state = AppState::new(session_id);
        let mut config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        config.auto_compaction = AutoCompactionConfig {
            enabled: true,
            threshold_percent: 90,
        };
        state.session_config = Some(config.clone());
        state.base_session_config = Some(config);
        state
    }

    async fn wait_for_compaction_result_auto_success(
        event_store: Arc<dyn EventStore>,
        session_id: SessionId,
    ) -> bool {
        timeout(Duration::from_secs(3), async {
            loop {
                let events = event_store
                    .load_events(session_id)
                    .await
                    .expect("load events");
                if events.iter().any(|(_, event)| {
                    matches!(
                        event,
                        SessionEvent::CompactResult {
                            result: CompactResult::Success(_),
                            trigger: CompactTrigger::Auto,
                        }
                    )
                }) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    async fn wait_for_no_error_event(
        event_store: Arc<dyn EventStore>,
        session_id: SessionId,
    ) -> bool {
        timeout(Duration::from_secs(3), async {
            loop {
                let events = event_store
                    .load_events(session_id)
                    .await
                    .expect("load events");
                if events
                    .iter()
                    .any(|(_, event)| matches!(event, SessionEvent::Error { .. }))
                {
                    return false;
                }
                if events.iter().any(|(_, event)| {
                    matches!(
                        event,
                        SessionEvent::CompactResult {
                            result: CompactResult::Success(_),
                            trigger: CompactTrigger::Auto,
                        }
                    )
                }) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or(false)
    }

    async fn dispatch_and_assert_ok(handle: &SessionActorHandle, action: Action) {
        handle.dispatch(action).await.expect("dispatch action");
    }

    async fn wait_for_operation_completed(
        event_store: Arc<dyn EventStore>,
        session_id: SessionId,
        op_id: OpId,
    ) -> bool {
        let result = timeout(Duration::from_secs(2), async {
            loop {
                let events = event_store
                    .load_events(session_id)
                    .await
                    .expect("load events");
                if events.iter().any(|(_, event)| {
                    matches!(
                        event,
                        SessionEvent::OperationCompleted { op_id: completed } if *completed == op_id
                    )
                }) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        result.is_ok()
    }

    #[tokio::test]
    async fn call_model_effect_dispatches_usage_with_completion_action() {
        let session_id = SessionId::new();
        let state = AppState::new(session_id);
        let (event_store, api_client, tool_executor) = create_test_deps().await;

        let model_id = crate::config::model::builtin::default_model();
        let provider_id = model_id.provider.clone();
        api_client.insert_test_provider(provider_id, Arc::new(StubProviderWithUsage));

        let mut actor =
            SessionActor::new(session_id, state, event_store, api_client, tool_executor);
        let op_id = OpId::new();

        actor
            .handle_effect(Effect::CallModel {
                session_id,
                op_id,
                model: model_id,
                messages: vec![],
                system_context: None,
                tools: vec![],
            })
            .await
            .expect("call model effect should succeed");

        let action = timeout(Duration::from_secs(2), actor.internal_action_rx.recv())
            .await
            .expect("timed out waiting for internal action")
            .expect("expected completion action");

        match action {
            Action::ModelResponseComplete {
                op_id: completed_op_id,
                usage,
                content,
                context_window_tokens,
                configured_max_output_tokens,
                ..
            } => {
                assert_eq!(completed_op_id, op_id);
                assert_eq!(usage, Some(TokenUsage::new(11, 13, 24)));
                assert_eq!(context_window_tokens, Some(400_000));
                assert!(configured_max_output_tokens.is_some());
                assert!(matches!(
                    content.as_slice(),
                    [AssistantContent::Text { text }] if text == "reply"
                ));
            }
            other => panic!("expected ModelResponseComplete, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_title_generation_uses_operation_cancel_token() {
        let session_id = SessionId::new();
        let mut state = AppState::new(session_id);
        let model_id = ModelId::new(ProviderId("blocking-stream".to_string()), "blocking-model");

        state.start_operation(OpId::new(), OperationKind::AgentLoop);
        state.session_config = Some(crate::session::state::SessionConfig {
            workspace: crate::session::state::WorkspaceConfig::default(),
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
            default_model: model_id.clone(),
            auto_compaction: crate::session::state::AutoCompactionConfig::default(),
        });

        let (event_store, api_client, tool_executor) = create_test_deps().await;
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        api_client.insert_test_provider(
            model_id.provider.clone(),
            Arc::new(BlockingStreamProvider {
                release_rx: Arc::new(tokio::sync::Mutex::new(Some(release_rx))),
            }),
        );

        let mut actor =
            SessionActor::new(session_id, state, event_store, api_client, tool_executor);
        let op_id = actor
            .state
            .current_operation
            .as_ref()
            .expect("operation should exist")
            .op_id;

        actor
            .handle_effect(Effect::GenerateSessionTitle {
                session_id,
                op_id,
                model: model_id,
                user_prompt: "Summarize this task".to_string(),
            })
            .await
            .expect("title generation effect should dispatch");

        actor
            .handle_effect(Effect::CancelOperation { session_id, op_id })
            .await
            .expect("cancel effect should succeed");

        let _ = release_tx.send(());

        let action = timeout(Duration::from_secs(2), actor.internal_action_rx.recv())
            .await
            .expect("timed out waiting for title action")
            .expect("expected title generation action");

        assert!(matches!(
            action,
            Action::SessionTitleGenerationFailed { .. }
        ));
    }

    #[tokio::test]
    async fn compaction_emits_operation_completed_event() {
        let session_id = SessionId::new();
        let mut state = AppState::new(session_id);
        seed_messages(&mut state);

        let (event_store, api_client, tool_executor) = create_test_deps().await;
        let provider_id = ProviderId("stub".to_string());
        let model_id = ModelId::new(provider_id.clone(), "stub-model");
        api_client.insert_test_provider(provider_id, Arc::new(StubProvider));

        let handle = spawn_session_actor(
            session_id,
            state,
            event_store.clone(),
            api_client,
            tool_executor,
        );

        let op_id = OpId::new();
        handle
            .dispatch(Action::RequestCompaction {
                session_id,
                op_id,
                model: model_id,
            })
            .await
            .expect("dispatch compaction");

        let completed = wait_for_operation_completed(event_store, session_id, op_id).await;
        handle.shutdown();

        assert!(
            completed,
            "expected OperationCompleted to be emitted for compaction"
        );
    }

    #[test]
    fn drop_stale_read_file_results_drops_old_reads_and_keeps_latest_per_file() {
        let messages = vec![
            Message {
                data: MessageData::Assistant {
                    content: vec![AssistantContent::ToolCall {
                        tool_call: ToolCall {
                            name: steer_tools::tools::VIEW_TOOL_NAME.to_string(),
                            parameters: json!({"file_path": "/tmp/a.rs", "offset": 1, "limit": 50}),
                            id: "read-a-1".to_string(),
                        },
                        thought_signature: None,
                    }],
                },
                id: "a-read-1".to_string(),
                parent_message_id: None,
                timestamp: 1,
            },
            Message {
                data: MessageData::Tool {
                    tool_use_id: "read-a-1".to_string(),
                    result: crate::app::conversation::ToolResult::FileContent(FileContentResult {
                        content: "first chunk".to_string(),
                        file_path: "/tmp/a.rs".to_string(),
                        line_count: 50,
                        truncated: true,
                    }),
                },
                id: "t-read-1".to_string(),
                parent_message_id: Some("a-read-1".to_string()),
                timestamp: 2,
            },
            Message {
                data: MessageData::Assistant {
                    content: vec![AssistantContent::ToolCall {
                        tool_call: ToolCall {
                            name: steer_tools::tools::VIEW_TOOL_NAME.to_string(),
                            parameters: json!({"file_path": "/tmp/a.rs", "offset": 20, "limit": 50}),
                            id: "read-a-2".to_string(),
                        },
                        thought_signature: None,
                    }],
                },
                id: "a-read-2".to_string(),
                parent_message_id: Some("t-read-1".to_string()),
                timestamp: 3,
            },
            Message {
                data: MessageData::Tool {
                    tool_use_id: "read-a-2".to_string(),
                    result: crate::app::conversation::ToolResult::FileContent(FileContentResult {
                        content: "second chunk".to_string(),
                        file_path: "/tmp/a.rs".to_string(),
                        line_count: 50,
                        truncated: false,
                    }),
                },
                id: "t-read-2".to_string(),
                parent_message_id: Some("a-read-2".to_string()),
                timestamp: 4,
            },
            Message {
                data: MessageData::Assistant {
                    content: vec![AssistantContent::ToolCall {
                        tool_call: ToolCall {
                            name: steer_tools::tools::EDIT_TOOL_NAME.to_string(),
                            parameters: json!({
                                "file_path": "/tmp/a.rs",
                                "old_string": "old",
                                "new_string": "new"
                            }),
                            id: "edit-a-1".to_string(),
                        },
                        thought_signature: None,
                    }],
                },
                id: "a-edit".to_string(),
                parent_message_id: Some("t-read-2".to_string()),
                timestamp: 5,
            },
            Message {
                data: MessageData::Tool {
                    tool_use_id: "edit-a-1".to_string(),
                    result: crate::app::conversation::ToolResult::Edit(EditResult {
                        file_path: "/tmp/a.rs".to_string(),
                        changes_made: 1,
                        file_created: false,
                        old_content: None,
                        new_content: None,
                    }),
                },
                id: "t-edit".to_string(),
                parent_message_id: Some("a-edit".to_string()),
                timestamp: 6,
            },
            Message {
                data: MessageData::Assistant {
                    content: vec![AssistantContent::ToolCall {
                        tool_call: ToolCall {
                            name: steer_tools::tools::VIEW_TOOL_NAME.to_string(),
                            parameters: json!({"file_path": "/tmp/a.rs", "offset": 1, "limit": 200}),
                            id: "read-a-3".to_string(),
                        },
                        thought_signature: None,
                    }],
                },
                id: "a-read-3".to_string(),
                parent_message_id: Some("t-edit".to_string()),
                timestamp: 7,
            },
            Message {
                data: MessageData::Tool {
                    tool_use_id: "read-a-3".to_string(),
                    result: crate::app::conversation::ToolResult::FileContent(FileContentResult {
                        content: "post-edit full".to_string(),
                        file_path: "/tmp/a.rs".to_string(),
                        line_count: 200,
                        truncated: false,
                    }),
                },
                id: "t-read-3".to_string(),
                parent_message_id: Some("a-read-3".to_string()),
                timestamp: 8,
            },
        ];

        let mut messages = messages;
        let dropped = drop_stale_read_file_results(&mut messages);
        assert_eq!(dropped, 2, "expected stale reads to be pruned");

        let remaining_tool_ids: Vec<String> = messages
            .iter()
            .filter_map(|message| {
                if let MessageData::Tool { tool_use_id, .. } = &message.data {
                    Some(tool_use_id.clone())
                } else {
                    None
                }
            })
            .collect();

        assert!(remaining_tool_ids.contains(&"read-a-3".to_string()));
        assert!(!remaining_tool_ids.contains(&"read-a-1".to_string()));
        assert!(!remaining_tool_ids.contains(&"read-a-2".to_string()));

        for message in &messages {
            if let MessageData::Assistant { content } = &message.data {
                for block in content {
                    if let AssistantContent::ToolCall { tool_call, .. } = block {
                        assert!(
                            tool_call.id != "read-a-1" && tool_call.id != "read-a-2",
                            "assistant tool call should be pruned alongside dropped result"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn drop_tool_results_with_matching_tool_calls_removes_orphan_assistant_messages() {
        let mut messages = vec![
            Message {
                data: MessageData::Assistant {
                    content: vec![
                        AssistantContent::Thought {
                            thought: ThoughtContent::Simple {
                                text: "thinking".to_string(),
                            },
                        },
                        AssistantContent::ToolCall {
                            tool_call: ToolCall {
                                name: steer_tools::tools::VIEW_TOOL_NAME.to_string(),
                                parameters: json!({"file_path": "/tmp/a.rs"}),
                                id: "read-1".to_string(),
                            },
                            thought_signature: None,
                        },
                    ],
                },
                id: "assistant-1".to_string(),
                parent_message_id: None,
                timestamp: 1,
            },
            Message {
                data: MessageData::Tool {
                    tool_use_id: "read-1".to_string(),
                    result: crate::app::conversation::ToolResult::FileContent(FileContentResult {
                        content: "content".to_string(),
                        file_path: "/tmp/a.rs".to_string(),
                        line_count: 1,
                        truncated: false,
                    }),
                },
                id: "tool-1".to_string(),
                parent_message_id: Some("assistant-1".to_string()),
                timestamp: 2,
            },
        ];

        let mut drop_ids = HashSet::new();
        drop_ids.insert("read-1".to_string());

        let dropped = drop_tool_results_with_matching_tool_calls(&mut messages, &drop_ids);
        assert_eq!(dropped, 1);
        assert!(
            messages.is_empty(),
            "assistant message with only thought + dropped tool call should be removed"
        );
    }

    #[tokio::test]
    async fn repeated_large_tool_results_trigger_auto_compaction_recovery_without_error() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let cargo_lock_path = temp_dir.path().join("Cargo.lock");
        let large_line = "x".repeat(120);
        let large_body = std::iter::repeat_n(large_line.as_str(), 12_000)
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(&cargo_lock_path, large_body)
            .await
            .expect("write large Cargo.lock test file");

        let session_id = SessionId::new();
        let state = auto_compact_test_state(session_id);

        let (event_store, api_client, _tool_executor) = create_test_deps().await;
        let model_id = builtin::claude_sonnet_4_5();
        let provider_id = model_id.provider.clone();
        api_client.insert_test_provider(
            provider_id,
            Arc::new(RepeatedReadThenOverflowProvider {
                file_path: cargo_lock_path.to_string_lossy().to_string(),
                overflow_char_limit: 120_000,
            }),
        );

        let workspace =
            crate::workspace::create_workspace(&crate::workspace::WorkspaceConfig::Local {
                path: temp_dir.path().to_path_buf(),
            })
            .await
            .expect("create test workspace");
        let model_registry = Arc::new(ModelRegistry::load(&[]).expect("model registry"));
        let tool_executor = ToolSystemBuilder::new(
            workspace,
            event_store.clone(),
            api_client.clone(),
            model_registry,
        )
        .with_backend_registry(Arc::new(BackendRegistry::new()))
        .with_validators(Arc::new(ValidatorRegistry::new()))
        .build();

        let handle = spawn_session_actor(
            session_id,
            state,
            event_store.clone(),
            api_client,
            tool_executor,
        );

        let user_op_id = OpId::new();
        let user_message_id = MessageId::new();
        dispatch_and_assert_ok(
            &handle,
            Action::UserInput {
                session_id,
                content: vec![UserContent::Text {
                    text: "repeatedly read large file chunks until we fill the context window"
                        .to_string(),
                }],
                op_id: user_op_id,
                message_id: user_message_id,
                model: model_id,
                timestamp: 1,
            },
        )
        .await;

        let compacted =
            wait_for_compaction_result_auto_success(event_store.clone(), session_id).await;
        let no_error = wait_for_no_error_event(event_store.clone(), session_id).await;

        handle.shutdown();

        assert!(
            compacted,
            "expected CompactResult::Success with auto trigger"
        );
        assert!(
            no_error,
            "expected no SessionEvent::Error after context-overflow recovery"
        );
    }

    #[tokio::test]
    async fn compaction_retries_when_context_window_is_exceeded() {
        let session_id = SessionId::new();
        let mut state = AppState::new(session_id);
        seed_messages_with_tool_results(&mut state, 4);

        let (event_store, api_client, tool_executor) = create_test_deps().await;
        let provider_id = ProviderId("stub".to_string());
        let model_id = ModelId::new(provider_id.clone(), "stub-model");
        let observed_tool_message_counts = Arc::new(std::sync::Mutex::new(Vec::new()));
        api_client.insert_test_provider(
            provider_id,
            Arc::new(ContextWindowLimitProvider {
                max_tool_messages: 1,
                observed_tool_message_counts: observed_tool_message_counts.clone(),
            }),
        );

        let handle = spawn_session_actor(
            session_id,
            state,
            event_store.clone(),
            api_client,
            tool_executor,
        );

        let op_id = OpId::new();
        handle
            .dispatch(Action::RequestCompaction {
                session_id,
                op_id,
                model: model_id,
            })
            .await
            .expect("dispatch compaction");

        let completed = wait_for_operation_completed(event_store.clone(), session_id, op_id).await;
        handle.shutdown();

        assert!(
            completed,
            "expected OperationCompleted to be emitted for compaction"
        );

        let counts = observed_tool_message_counts
            .lock()
            .expect("lock observed tool message counts")
            .clone();
        assert!(
            counts.len() >= 2,
            "expected retry after context limit error; observed counts: {counts:?}"
        );
        assert!(
            counts.windows(2).any(|pair| pair[1] < pair[0]),
            "expected tool message counts to decrease across retries; observed counts: {counts:?}"
        );
        assert!(
            counts.last().is_some_and(|count| *count <= 1),
            "expected final retry to fit provider limit; observed counts: {counts:?}"
        );

        let events = event_store
            .load_events(session_id)
            .await
            .expect("load events after compaction");
        assert!(events.iter().any(|(_, event)| {
            matches!(
                event,
                SessionEvent::CompactResult {
                    result: crate::app::domain::event::CompactResult::Success(_),
                    ..
                }
            )
        }));
        assert!(!events.iter().any(|(_, event)| {
            matches!(
                event,
                SessionEvent::CompactResult {
                    result: crate::app::domain::event::CompactResult::Failed(_),
                    ..
                }
            )
        }));
    }
}
