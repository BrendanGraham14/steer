use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::api::Client as ApiClient;
use crate::app::domain::action::Action;
use crate::app::domain::event::SessionEvent;
use crate::app::domain::session::EventStore;
use crate::app::domain::types::{NonEmptyString, OpId, RequestId, SessionId};
use crate::config::model::ModelId;
use crate::tools::ToolExecutor;

use super::runtime::{AppRuntime, RuntimeConfig, RuntimeError};

pub struct RuntimeManagedSession {
    session_id: SessionId,
    action_tx: mpsc::Sender<Action>,
    event_rx: Option<mpsc::Receiver<SessionEvent>>,
    runtime_task: JoinHandle<()>,
    last_activity: chrono::DateTime<chrono::Utc>,
}

impl RuntimeManagedSession {
    pub async fn new(
        api_client: Arc<ApiClient>,
        tool_executor: Arc<ToolExecutor>,
        event_store: Arc<dyn EventStore>,
        default_model: ModelId,
    ) -> Result<Self, RuntimeError> {
        let (action_tx, mut action_rx) = mpsc::channel::<Action>(32);
        let (event_tx, event_rx) = mpsc::channel::<(SessionId, SessionEvent)>(256);
        let (session_event_tx, session_event_rx) = mpsc::channel::<SessionEvent>(256);

        let config = RuntimeConfig::new(default_model);
        let mut runtime =
            AppRuntime::new(event_store, api_client, tool_executor, config, event_tx)?;

        let session_id = runtime.create_session().await?;

        let runtime_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(action) = action_rx.recv() => {
                        if matches!(action, Action::Shutdown) {
                            break;
                        }
                        if let Err(e) = dispatch_action(&mut runtime, action).await {
                            tracing::error!("Error dispatching action: {}", e);
                        }
                    }
                    else => break,
                }
            }
        });

        let forward_session_id = session_id;
        tokio::spawn(async move {
            let mut event_rx = event_rx;
            while let Some((sid, event)) = event_rx.recv().await {
                if sid == forward_session_id {
                    if session_event_tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            session_id,
            action_tx,
            event_rx: Some(session_event_rx),
            runtime_task,
            last_activity: chrono::Utc::now(),
        })
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub async fn submit_user_input(
        &self,
        text: String,
        model: ModelId,
    ) -> Result<OpId, RuntimeError> {
        let text = NonEmptyString::new(text).ok_or_else(|| RuntimeError::InvalidInput {
            message: "Input text cannot be empty".to_string(),
        })?;

        let op_id = OpId::new();
        let message_id = crate::app::domain::types::MessageId::new();
        let timestamp = current_timestamp();

        let action = Action::UserInput {
            session_id: self.session_id,
            text,
            op_id,
            message_id,
            model,
            timestamp,
        };

        self.action_tx
            .send(action)
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;

        Ok(op_id)
    }

    pub async fn submit_tool_approval(
        &self,
        request_id: RequestId,
        approved: bool,
        remember_tool: Option<String>,
        remember_pattern: Option<String>,
    ) -> Result<(), RuntimeError> {
        use crate::app::domain::action::{ApprovalDecision, ApprovalMemory};

        let decision = if approved {
            ApprovalDecision::Approved
        } else {
            ApprovalDecision::Denied
        };

        let remember = if let Some(tool) = remember_tool {
            Some(ApprovalMemory::Tool(tool))
        } else if let Some(pattern) = remember_pattern {
            Some(ApprovalMemory::BashPattern(pattern))
        } else {
            None
        };

        let action = Action::ToolApprovalDecided {
            session_id: self.session_id,
            request_id,
            decision,
            remember,
        };

        self.action_tx
            .send(action)
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;

        Ok(())
    }

    pub async fn cancel(&self) -> Result<(), RuntimeError> {
        let action = Action::Cancel {
            session_id: self.session_id,
            op_id: None,
        };

        self.action_tx
            .send(action)
            .await
            .map_err(|_| RuntimeError::ChannelClosed)?;

        Ok(())
    }

    pub fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<SessionEvent>> {
        self.event_rx.take()
    }

    pub fn touch(&mut self) {
        self.last_activity = chrono::Utc::now();
    }

    pub fn last_activity(&self) -> chrono::DateTime<chrono::Utc> {
        self.last_activity
    }

    pub async fn shutdown(self) {
        let _ = self.action_tx.send(Action::Shutdown).await;
        let _ = self.runtime_task.await;
    }
}

async fn dispatch_action(runtime: &mut AppRuntime, action: Action) -> Result<(), RuntimeError> {
    let session_id = action
        .session_id()
        .ok_or_else(|| RuntimeError::InvalidInput {
            message: "Action has no session_id".to_string(),
        })?;

    runtime.dispatch_action(session_id, action).await
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
