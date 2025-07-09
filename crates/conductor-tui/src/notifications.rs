//! Notification module for the TUI
//!
//! Provides sound and desktop notifications when processing completes.

use crate::error::Result;
use notify_rust::Notification;
use process_wrap::tokio::{ProcessGroup, TokioCommandWrap};
use std::fmt;
use std::str::FromStr;
use std::time::Duration;
use tokio::time::sleep;
use tracing::debug;

/// Type of notification sound to play
#[derive(Debug, Clone, Copy)]
pub enum NotificationSound {
    /// Processing complete - ascending tones
    ProcessingComplete,
    /// Tool approval needed - urgent double beep
    ToolApproval,
    /// Error occurred - descending tones
    Error,
}

impl FromStr for NotificationSound {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "ProcessingComplete" => Ok(NotificationSound::ProcessingComplete),
            "ToolApproval" => Ok(NotificationSound::ToolApproval),
            "Error" => Ok(NotificationSound::Error),
            _ => Err(()),
        }
    }
}

impl fmt::Display for NotificationSound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            NotificationSound::ProcessingComplete => "ProcessingComplete",
            NotificationSound::ToolApproval => "ToolApproval",
            NotificationSound::Error => "Error",
        };
        write!(f, "{s}")
    }
}

/// Get the appropriate system sound name for the notification type
fn get_sound_name(sound_type: NotificationSound) -> &'static str {
    #[cfg(target_os = "macos")]
    {
        match sound_type {
            NotificationSound::ProcessingComplete => "Glass", // Pleasant completion sound
            NotificationSound::ToolApproval => "Ping",        // Attention-getting sound
            NotificationSound::Error => "Basso",              // Error/failure sound
        }
    }

    #[cfg(target_os = "linux")]
    {
        match sound_type {
            NotificationSound::ProcessingComplete => "message-new-instant", // Completion sound
            NotificationSound::ToolApproval => "dialog-warning", // Warning/attention sound
            NotificationSound::Error => "dialog-error",          // Error sound
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows has limited notification sound options
        match sound_type {
            NotificationSound::ProcessingComplete => "default",
            NotificationSound::ToolApproval => "default",
            NotificationSound::Error => "default",
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "default"
    }
}

/// Show a desktop notification with optional sound
pub fn show_notification_with_sound(
    title: &str,
    message: &str,
    sound_type: Option<NotificationSound>,
) -> Result<()> {
    let mut notification = Notification::new();
    notification
        .summary(title)
        .body(message)
        .appname("conductor")
        .timeout(5000);

    // Add sound if specified
    if let Some(sound) = sound_type {
        notification.sound_name(get_sound_name(sound));
    }

    #[cfg(target_os = "linux")]
    {
        notification.icon("terminal").timeout(5000);
    }

    notification.show()?;
    Ok(())
}

/// Configuration for notifications
#[derive(Debug, Clone)]
pub struct NotificationConfig {
    pub enable_sound: bool,
    pub enable_desktop_notification: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enable_sound: true,
            enable_desktop_notification: true,
        }
    }
}

impl NotificationConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            enable_sound: std::env::var("CONDUCTOR_NOTIFICATION_SOUND")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
            enable_desktop_notification: std::env::var("CONDUCTOR_NOTIFICATION_DESKTOP")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
        }
    }
}

/// We need to do this in a subprocess because on mac at least, notify-rust's Notification::show()
/// **NEVER RETURNS**.
/// This is a workaround to ensure that both:
/// 1. The notification is shown
/// 2. We don't leak tokio tasks / threads
/// 3. We don't end up with blocking tokio tasks which prevent the main thread from exiting.
async fn trigger_notification_subprocess(
    title: &str,
    message: &str,
    sound: Option<NotificationSound>,
) -> Result<()> {
    let current_exe = std::env::current_exe()?;
    let mut args = vec![
        "notify".to_string(),
        "--title".to_string(),
        title.to_string(),
        "--message".to_string(),
        message.to_string(),
    ];

    if let Some(sound_type) = sound {
        args.push("--sound".to_string());
        args.push(sound_type.to_string());
    }

    let mut child = TokioCommandWrap::with_new(current_exe, |command| {
        command.args(args);
    })
    .wrap(ProcessGroup::leader())
    .spawn()?;

    tokio::spawn(async move {
        sleep(Duration::from_secs(2)).await;
        match child.start_kill() {
            Ok(_) => {}
            Err(e) => {
                debug!("Failed to kill notification subprocess: {}", e);
            }
        }
    });

    Ok(())
}

/// Trigger notifications with specific sound
pub async fn notify_with_sound(
    config: &NotificationConfig,
    sound: NotificationSound,
    message: &str,
) {
    notify_with_title_and_sound(config, sound, "Conductor", message).await;
}

/// Trigger notifications with custom title and sound
pub async fn notify_with_title_and_sound(
    config: &NotificationConfig,
    sound: NotificationSound,
    title: &str,
    message: &str,
) {
    if config.enable_desktop_notification {
        let sound_option = if config.enable_sound {
            Some(sound)
        } else {
            None
        };
        match trigger_notification_subprocess(title, message, sound_option).await {
            Ok(_) => {}
            Err(e) => {
                debug!("Failed to trigger notification subprocess: {}", e);
            }
        }
    }
}
