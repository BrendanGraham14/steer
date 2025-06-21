use anyhow::{anyhow, Result};
use async_trait::async_trait;

use crate::app::command::AppCommand;
use crate::app::io::AppCommandSink;

/// Simple wrapper around `mpsc::Sender<AppCommand>` that implements
/// the `AppCommandSink` trait so the TUI can work with a trait object.
pub struct CommandSender {
    tx: tokio::sync::mpsc::Sender<AppCommand>,
}

impl CommandSender {
    pub fn new(tx: tokio::sync::mpsc::Sender<AppCommand>) -> Self {
        Self { tx }
    }

    /// Access to the inner sender – used temporarily during the migration
    pub fn inner(&self) -> &tokio::sync::mpsc::Sender<AppCommand> {
        &self.tx
    }
}

#[async_trait]
impl AppCommandSink for CommandSender {
    async fn send_command(&self, command: AppCommand) -> Result<()> {
        self.tx
            .send(command)
            .await
            .map_err(|e| anyhow!("Failed to send command: {}", e))
    }
}
