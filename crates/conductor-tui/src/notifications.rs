//! Notification module for the TUI
//!
//! Provides sound and desktop notifications when processing completes.

use anyhow::Result;
use notify_rust::Notification;
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
            NotificationSound::ToolApproval => "dialog-warning",           // Warning/attention sound
            NotificationSound::Error => "dialog-error",                    // Error sound
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
pub fn show_notification_with_sound(title: &str, message: &str, sound_type: Option<NotificationSound>) -> Result<()> {
    let mut notification = Notification::new();
    notification
        .summary(title)
        .body(message)
        .appname("conductor");

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

/// Trigger notifications with specific sound
pub fn notify_with_sound(config: &NotificationConfig, sound: NotificationSound, message: &str) {
    if config.enable_desktop_notification {
        let sound_option = if config.enable_sound { Some(sound) } else { None };
        if let Err(e) = show_notification_with_sound("Conductor", message, sound_option) {
            debug!("Failed to show desktop notification: {}", e);
        }
    }
}

/// Trigger notifications with custom title and sound
pub fn notify_with_title_and_sound(
    config: &NotificationConfig,
    sound: NotificationSound,
    title: &str,
    message: &str,
) {
    if config.enable_desktop_notification {
        let sound_option = if config.enable_sound { Some(sound) } else { None };
        if let Err(e) = show_notification_with_sound(title, message, sound_option) {
            debug!("Failed to show desktop notification: {}", e);
        }
    }
}
