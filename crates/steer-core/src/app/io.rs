use crate::error::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use super::{AppCommand, AppEvent};

/// Abstraction over a channel or transport that accepts `AppCommand`s sent
/// from the UI or other front-ends.
#[async_trait]
pub trait AppCommandSink: Send + Sync {
    /// Send a command to the application core.
    async fn send_command(&self, command: AppCommand) -> Result<()>;
}

/// Source of [`AppEvent`]s produced by the application core that front-ends can
/// listen to for UI updates.
///
/// Implementations typically wrap an `mpsc::Receiver<AppEvent>` or forward
/// events from an external transport (e.g. gRPC stream).
#[async_trait]
pub trait AppEventSource: Send + Sync {
    /// Obtain a receiver that yields application events. A fresh receiver
    /// should be returned on every call so multiple consumers can coexist.
    async fn subscribe(&self) -> mpsc::Receiver<AppEvent>;
}
