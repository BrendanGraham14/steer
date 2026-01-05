use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{broadcast, mpsc};

use crate::app::domain::delta::StreamDelta;
use crate::app::domain::event::SessionEvent;
use crate::app::domain::types::{MessageId, OpId};

pub use crate::app::domain::types::SessionId;

const EVENT_CHANNEL_SIZE: usize = 256;
const EVENT_OVERFLOW_MAX: usize = 64;
const DELTA_CHANNEL_SIZE: usize = 1024;
const DELTA_COALESCE_MAX: usize = 32;

#[derive(Debug, Default)]
pub struct ChannelMetrics {
    pub events_sent: AtomicU64,
    pub events_evicted: AtomicU64,
    pub events_dropped: AtomicU64,
    pub overflow_used: AtomicU64,
    pub deltas_buffered: AtomicU64,
    pub deltas_sent: AtomicU64,
    pub deltas_dropped: AtomicU64,
}

impl ChannelMetrics {
    fn inc_events_sent(&self) {
        self.events_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_events_evicted(&self) {
        self.events_evicted.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_events_dropped(&self) {
        self.events_dropped.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_overflow_used(&self) {
        self.overflow_used.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_deltas_buffered(&self) {
        self.deltas_buffered.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_deltas_sent(&self) {
        self.deltas_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_deltas_dropped(&self) {
        self.deltas_dropped.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            events_sent: self.events_sent.load(Ordering::Relaxed),
            events_evicted: self.events_evicted.load(Ordering::Relaxed),
            events_dropped: self.events_dropped.load(Ordering::Relaxed),
            overflow_used: self.overflow_used.load(Ordering::Relaxed),
            deltas_buffered: self.deltas_buffered.load(Ordering::Relaxed),
            deltas_sent: self.deltas_sent.load(Ordering::Relaxed),
            deltas_dropped: self.deltas_dropped.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub events_sent: u64,
    pub events_evicted: u64,
    pub events_dropped: u64,
    pub overflow_used: u64,
    pub deltas_buffered: u64,
    pub deltas_sent: u64,
    pub deltas_dropped: u64,
}

pub struct DeltaCoalescer {
    pending: HashMap<(OpId, MessageId), String>,
    order: Vec<(OpId, MessageId)>,
    max_pending: usize,
}

impl DeltaCoalescer {
    pub fn new(max_pending: usize) -> Self {
        Self {
            pending: HashMap::new(),
            order: Vec::new(),
            max_pending,
        }
    }

    pub fn push(&mut self, delta: StreamDelta) {
        if let StreamDelta::TextChunk {
            op_id,
            message_id,
            delta: text,
            ..
        } = delta
        {
            let key = (op_id, message_id.clone());
            if let Some(existing) = self.pending.get_mut(&key) {
                existing.push_str(&text);
            } else {
                if self.pending.len() >= self.max_pending {
                    if let Some(oldest_key) = self.order.first().cloned() {
                        self.pending.remove(&oldest_key);
                        self.order.remove(0);
                    }
                }
                self.pending.insert(key.clone(), text);
                self.order.push(key);
            }
        }
    }

    pub fn drain(&mut self) -> Vec<StreamDelta> {
        let mut result = Vec::new();
        for (op_id, message_id) in self.order.drain(..) {
            if let Some(text) = self.pending.remove(&(op_id, message_id.clone())) {
                result.push(StreamDelta::TextChunk {
                    op_id,
                    message_id,
                    delta: text,
                    is_first: false,
                });
            }
        }
        result
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

pub struct DualChannelDispatcher {
    event_tx: mpsc::Sender<(SessionId, SessionEvent)>,
    event_overflow: VecDeque<(SessionId, SessionEvent)>,
    delta_tx: broadcast::Sender<StreamDelta>,
    delta_buffer: DeltaCoalescer,
    metrics: ChannelMetrics,
}

impl DualChannelDispatcher {
    pub fn new() -> (Self, mpsc::Receiver<(SessionId, SessionEvent)>, broadcast::Receiver<StreamDelta>) {
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_SIZE);
        let (delta_tx, delta_rx) = broadcast::channel(DELTA_CHANNEL_SIZE);

        let dispatcher = Self {
            event_tx,
            event_overflow: VecDeque::new(),
            delta_tx,
            delta_buffer: DeltaCoalescer::new(DELTA_COALESCE_MAX),
            metrics: ChannelMetrics::default(),
        };

        (dispatcher, event_rx, delta_rx)
    }

    pub fn subscribe_deltas(&self) -> broadcast::Receiver<StreamDelta> {
        self.delta_tx.subscribe()
    }

    pub fn dispatch_event(&mut self, session_id: SessionId, event: SessionEvent) {
        match self.event_tx.try_send((session_id, event.clone())) {
            Ok(_) => {
                self.metrics.inc_events_sent();
                self.drain_overflow();
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.handle_overflow(session_id, event);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.metrics.inc_events_dropped();
                tracing::error!("Event channel closed");
            }
        }
    }

    fn handle_overflow(&mut self, session_id: SessionId, event: SessionEvent) {
        if self.event_overflow.len() >= EVENT_OVERFLOW_MAX {
            if let Some(pos) = self
                .event_overflow
                .iter()
                .position(|(_, e)| !matches!(e, SessionEvent::Error { .. }))
            {
                self.event_overflow.remove(pos);
            } else {
                self.event_overflow.pop_front();
            }
            self.metrics.inc_events_evicted();
        }
        self.event_overflow.push_back((session_id, event));
        self.metrics.inc_overflow_used();
    }

    fn drain_overflow(&mut self) {
        while let Some((session_id, event)) = self.event_overflow.pop_front() {
            match self.event_tx.try_send((session_id, event.clone())) {
                Ok(_) => {
                    self.metrics.inc_events_sent();
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    self.event_overflow.push_front((session_id, event));
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.metrics.inc_events_dropped();
                    break;
                }
            }
        }
    }

    pub fn dispatch_delta(&mut self, delta: StreamDelta) {
        self.delta_buffer.push(delta);
        self.metrics.inc_deltas_buffered();
    }

    pub fn flush_deltas(&mut self) {
        for delta in self.delta_buffer.drain() {
            match self.delta_tx.send(delta) {
                Ok(_) => self.metrics.inc_deltas_sent(),
                Err(_) => self.metrics.inc_deltas_dropped(),
            }
        }
    }

    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }

    pub fn overflow_len(&self) -> usize {
        self.event_overflow.len()
    }
}

impl Default for DualChannelDispatcher {
    fn default() -> Self {
        Self::new().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_coalescer_combines_chunks() {
        let mut coalescer = DeltaCoalescer::new(10);
        let op_id = OpId::new();
        let message_id = MessageId::new();

        coalescer.push(StreamDelta::TextChunk {
            op_id,
            message_id: message_id.clone(),
            delta: "Hello ".to_string(),
            is_first: true,
        });

        coalescer.push(StreamDelta::TextChunk {
            op_id,
            message_id: message_id.clone(),
            delta: "World".to_string(),
            is_first: false,
        });

        let result = coalescer.drain();
        assert_eq!(result.len(), 1);

        if let StreamDelta::TextChunk { delta, .. } = &result[0] {
            assert_eq!(delta, "Hello World");
        } else {
            panic!("Expected TextChunk");
        }
    }

    #[test]
    fn test_delta_coalescer_respects_max() {
        let mut coalescer = DeltaCoalescer::new(2);

        for i in 0..3 {
            let op_id = OpId::new();
            let message_id = MessageId::new();
            coalescer.push(StreamDelta::TextChunk {
                op_id,
                message_id,
                delta: format!("chunk {}", i),
                is_first: i == 0,
            });
        }

        assert_eq!(coalescer.pending.len(), 2);
    }

    #[tokio::test]
    async fn test_dispatcher_sends_events() {
        let (mut dispatcher, mut event_rx, _delta_rx) = DualChannelDispatcher::new();

        let session_id = SessionId::new();
        dispatcher.dispatch_event(
            session_id,
            SessionEvent::Error {
                message: "test".to_string(),
            },
        );

        let received = event_rx.recv().await.unwrap();
        assert_eq!(received.0, session_id);
    }

    #[tokio::test]
    async fn test_dispatcher_overflow_preserves_errors() {
        let (event_tx, _event_rx) = mpsc::channel(1);
        let (delta_tx, _) = broadcast::channel(16);

        let mut dispatcher = DualChannelDispatcher {
            event_tx,
            event_overflow: VecDeque::new(),
            delta_tx,
            delta_buffer: DeltaCoalescer::new(10),
            metrics: ChannelMetrics::default(),
        };

        let session_id = SessionId::new();

        dispatcher.dispatch_event(
            session_id,
            SessionEvent::OperationCompleted { op_id: OpId::new() },
        );

        dispatcher.dispatch_event(session_id, SessionEvent::Error { message: "keep".to_string() });

        for _ in 0..EVENT_OVERFLOW_MAX + 5 {
            dispatcher.dispatch_event(
                session_id,
                SessionEvent::OperationCompleted { op_id: OpId::new() },
            );
        }

        let has_error = dispatcher
            .event_overflow
            .iter()
            .any(|(_, e)| matches!(e, SessionEvent::Error { .. }));
        assert!(has_error, "Error event should be preserved in overflow");
    }
}
