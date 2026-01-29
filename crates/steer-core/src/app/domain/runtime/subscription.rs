use tokio::sync::{broadcast, mpsc};

use crate::app::domain::event::SessionEvent;
use crate::app::domain::types::SessionId;

#[derive(Debug, Clone)]
pub struct SessionEventEnvelope {
    pub seq: u64,
    pub event: SessionEvent,
}

pub struct SessionEventSubscription {
    pub session_id: SessionId,
    pub rx: broadcast::Receiver<SessionEventEnvelope>,
    unsubscribe_tx: mpsc::UnboundedSender<UnsubscribeSignal>,
}

pub(crate) struct UnsubscribeSignal;

impl SessionEventSubscription {
    pub(crate) fn new(
        session_id: SessionId,
        rx: broadcast::Receiver<SessionEventEnvelope>,
        unsubscribe_tx: mpsc::UnboundedSender<UnsubscribeSignal>,
    ) -> Self {
        Self {
            session_id,
            rx,
            unsubscribe_tx,
        }
    }

    pub async fn recv(&mut self) -> Option<SessionEventEnvelope> {
        loop {
            match self.rx.recv().await {
                Ok(envelope) => return Some(envelope),
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        session_id = %self.session_id,
                        lagged = n,
                        "Event subscriber lagged, some events were dropped"
                    );
                }
            }
        }
    }
}

impl Drop for SessionEventSubscription {
    fn drop(&mut self) {
        let _ = self.unsubscribe_tx.send(UnsubscribeSignal);
    }
}
