use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

use crate::app::AppEvent;
use crate::app::command::AppCommand;
use crate::app::io::{AppCommandSink, AppEventSource};

/// Local adapter that implements AppCommandSink and AppEventSource for in-process communication
pub struct LocalAdapter {
    command_tx: mpsc::Sender<AppCommand>,
    event_rx: Arc<Mutex<Option<mpsc::Receiver<AppEvent>>>>,
}

impl LocalAdapter {
    pub fn new(command_tx: mpsc::Sender<AppCommand>, event_rx: mpsc::Receiver<AppEvent>) -> Self {
        Self {
            command_tx,
            event_rx: Arc::new(Mutex::new(Some(event_rx))),
        }
    }
}

#[async_trait]
impl AppCommandSink for LocalAdapter {
    async fn send_command(&self, command: AppCommand) -> Result<()> {
        self.command_tx
            .send(command)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send command: {}", e))
    }
}

#[async_trait]
impl AppEventSource for LocalAdapter {
    async fn subscribe(&self) -> mpsc::Receiver<AppEvent> {
        // This is a blocking operation in a trait that doesn't support async
        // We need to use block_on here
        self.event_rx
            .lock()
            .await
            .take()
            .expect("Event receiver already taken - LocalAdapter only supports single subscription")
    }
}
