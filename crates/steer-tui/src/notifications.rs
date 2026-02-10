//! Notification module for the TUI.
//!
//! Provides centralized, focus-aware notification delivery with OSC 9 as the
//! primary transport.

use ratatui::crossterm::{Command, execute};
use std::fmt;
use std::io::{self, stdout};
use std::sync::{Arc, Mutex};
use tracing::debug;

use steer_grpc::client_api::{NotificationTransport, Preferences};

/// High-level notification categories emitted by event processors.
#[derive(Debug, Clone)]
pub enum NotificationEvent {
    ProcessingComplete,
    ToolApprovalRequested { tool_name: String },
    Error { message: String },
}

impl NotificationEvent {
    fn title() -> &'static str {
        "Steer"
    }

    fn body(&self) -> String {
        match self {
            NotificationEvent::ProcessingComplete => {
                "Processing complete - waiting for input".to_string()
            }
            NotificationEvent::ToolApprovalRequested { tool_name } => {
                format!("Tool approval needed: {tool_name}")
            }
            NotificationEvent::Error { message } => message.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectiveTransport {
    Osc9,
    Off,
}

/// Focus-aware manager used by event processors to emit notifications.
#[derive(Debug)]
pub struct NotificationManager {
    inner: Mutex<NotificationState>,
}

#[derive(Debug)]
struct NotificationState {
    transport: EffectiveTransport,
    terminal_focused: bool,
    focus_events_enabled: bool,
}

impl NotificationManager {
    pub fn new(preferences: &Preferences) -> Self {
        let config = NotificationConfig::from_preferences(preferences);
        let transport = resolve_transport(config.transport);
        Self {
            inner: Mutex::new(NotificationState {
                transport,
                terminal_focused: true,
                focus_events_enabled: false,
            }),
        }
    }

    pub fn set_terminal_focused(&self, focused: bool) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.terminal_focused = focused;
    }

    pub fn set_focus_events_enabled(&self, enabled: bool) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.focus_events_enabled = enabled;
    }

    pub fn emit(&self, event: NotificationEvent) {
        let (should_emit, transport, title, body) = {
            let state = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let should_emit = if state.focus_events_enabled {
                !state.terminal_focused
            } else {
                true
            };
            (
                should_emit,
                state.transport,
                NotificationEvent::title().to_string(),
                event.body(),
            )
        };

        if !should_emit || transport == EffectiveTransport::Off {
            return;
        }

        match transport {
            EffectiveTransport::Osc9 => {
                if let Err(err) = show_osc9_notification(&title, &body) {
                    debug!("Failed to emit OSC 9 notification: {err}");
                }
            }
            EffectiveTransport::Off => {}
        }
    }
}

fn resolve_transport(transport: NotificationTransport) -> EffectiveTransport {
    match transport {
        NotificationTransport::Auto | NotificationTransport::Osc9 => EffectiveTransport::Osc9,
        NotificationTransport::Off => EffectiveTransport::Off,
    }
}

/// Shared handle passed through TUI state and event processors.
pub type NotificationManagerHandle = Arc<NotificationManager>;

/// Configuration for notifications.
#[derive(Debug, Clone)]
pub struct NotificationConfig {
    pub transport: NotificationTransport,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            transport: NotificationTransport::Auto,
        }
    }
}

impl NotificationConfig {
    pub fn from_preferences(preferences: &Preferences) -> Self {
        Self {
            transport: preferences.ui.notifications.transport,
        }
    }
}

/// Command that emits an OSC 9 notification with a message.
#[derive(Debug, Clone)]
struct PostNotification(pub String);

impl Command for PostNotification {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b]9;{}\x07", self.0)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(std::io::Error::other(
            "tried to execute PostNotification using WinAPI; use ANSI instead",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

fn show_osc9_notification(title: &str, message: &str) -> io::Result<()> {
    let body = if title.is_empty() {
        message.to_string()
    } else {
        format!("{title}: {message}")
    };
    execute!(stdout(), PostNotification(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use steer_grpc::client_api::NotificationTransport;

    fn prefs_with_transport(transport: NotificationTransport) -> Preferences {
        let mut prefs = Preferences::default();
        prefs.ui.notifications.transport = transport;
        prefs
    }

    #[test]
    fn resolve_auto_to_osc9() {
        let prefs = prefs_with_transport(NotificationTransport::Auto);
        let manager = NotificationManager::new(&prefs);
        let state = manager
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.transport, EffectiveTransport::Osc9);
    }

    #[test]
    fn resolve_off_to_off() {
        let prefs = prefs_with_transport(NotificationTransport::Off);
        let manager = NotificationManager::new(&prefs);
        let state = manager
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.transport, EffectiveTransport::Off);
    }

    #[test]
    fn event_body_formats_approval() {
        let body = NotificationEvent::ToolApprovalRequested {
            tool_name: "bash".to_string(),
        }
        .body();
        assert_eq!(body, "Tool approval needed: bash");
    }
}
