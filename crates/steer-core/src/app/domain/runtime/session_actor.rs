use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::domain::action::Action;
use crate::app::domain::effect::Effect;
use crate::app::domain::event::SessionEvent;
use crate::app::domain::reduce::reduce;
use crate::app::domain::session::{EventStore, EventStoreError};
use crate::app::domain::state::AppState;
use crate::app::domain::types::{MessageId, OpId, SessionId};
use crate::tools::ToolExecutor;

use super::interpreter::EffectInterpreter;
use super::subscription::{SessionEventEnvelope, SessionEventSubscription, UnsubscribeSignal};

const EVENT_BROADCAST_CAPACITY: usize = 256;

pub(crate) enum SessionCmd {
    Dispatch {
        action: Action,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    Subscribe {
        reply: oneshot::Sender<SessionEventSubscription>,
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
    pub session_id: SessionId,
    pub cmd_tx: mpsc::Sender<SessionCmd>,
    pub task: JoinHandle<()>,
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

    pub fn shutdown(&self) {
        let _ = self.cmd_tx.try_send(SessionCmd::Shutdown);
    }
}

struct SessionActor {
    session_id: SessionId,
    state: AppState,
    event_store: Arc<dyn EventStore>,
    interpreter: EffectInterpreter,
    active_operations: HashMap<OpId, CancellationToken>,
    event_broadcast: broadcast::Sender<SessionEventEnvelope>,
    subscriber_count: usize,
    unsubscribe_rx: mpsc::UnboundedReceiver<UnsubscribeSignal>,
    unsubscribe_tx: mpsc::UnboundedSender<UnsubscribeSignal>,
    internal_action_tx: mpsc::Sender<Action>,
    internal_action_rx: mpsc::Receiver<Action>,
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
        let (unsubscribe_tx, unsubscribe_rx) = mpsc::unbounded_channel();
        let (internal_action_tx, internal_action_rx) = mpsc::channel(64);
        let interpreter = EffectInterpreter::new(api_client, tool_executor);

        Self {
            session_id,
            state,
            event_store,
            interpreter,
            active_operations: HashMap::new(),
            event_broadcast,
            subscriber_count: 0,
            unsubscribe_rx,
            unsubscribe_tx,
            internal_action_tx,
            internal_action_rx,
        }
    }

    async fn run(mut self, mut cmd_rx: mpsc::Receiver<SessionCmd>) {
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
                        SessionCmd::GetState { reply } => {
                            let _ = reply.send(self.state.clone());
                        }
                        SessionCmd::Suspend { reply } => {
                            let _ = reply.send(());
                            break;
                        }
                        SessionCmd::Shutdown => {
                            self.cancel_all_operations();
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
                system_prompt,
                tools,
                ..
            } => {
                let cancel_token = self
                    .active_operations
                    .entry(op_id)
                    .or_insert_with(CancellationToken::new)
                    .clone();

                let interpreter = self.interpreter.clone();
                let action_tx = self.internal_action_tx.clone();
                let session_id = self.session_id;

                tokio::spawn(async move {
                    let result = interpreter
                        .call_model(model, messages, system_prompt, tools, cancel_token)
                        .await;

                    let action = match result {
                        Ok(content) => Action::ModelResponseComplete {
                            session_id,
                            op_id,
                            message_id: MessageId::new(),
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
                let cancel_token = self
                    .active_operations
                    .entry(op_id)
                    .or_insert_with(CancellationToken::new)
                    .clone();

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

            Effect::ConnectMcpServer { .. } | Effect::DisconnectMcpServer { .. } => Ok(()),
        }
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

    let task = tokio::spawn(actor.run(cmd_rx));

    SessionActorHandle {
        session_id,
        cmd_tx,
        task,
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
