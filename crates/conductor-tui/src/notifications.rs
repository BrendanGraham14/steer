//! Notification module for the TUI
//!
//! Provides sound and desktop notifications when processing completes.

use anyhow::Result;
use notify_rust::Notification;
use std::time::Duration;
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

/// Play a notification sound based on type
pub fn play_notification_sound(sound_type: NotificationSound) {
    // Spawn a thread to play sound without blocking
    std::thread::spawn(move || {
        match sound_type {
            NotificationSound::ProcessingComplete => {
                // Ascending pleasant tones
                for freq in [300, 450, 600] {
                    actually_beep::beep_with_hz_and_millis(freq, 40).unwrap_or_else(|e| {
                        debug!("Failed to beep at {}Hz: {}", freq, e);
                    });
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            NotificationSound::ToolApproval => {
                // Urgent double beep
                for _ in 0..2 {
                    actually_beep::beep_with_hz_and_millis(800, 50).unwrap_or_else(|e| {
                        debug!("Failed to beep: {}", e);
                    });
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            NotificationSound::Error => {
                // Descending tones
                for freq in [600, 450, 300] {
                    actually_beep::beep_with_hz_and_millis(freq, 40).unwrap_or_else(|e| {
                        debug!("Failed to beep at {}Hz: {}", freq, e);
                    });
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    });
}

/// Show a desktop notification
pub fn show_notification(title: &str, message: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Notification::new()
            .summary(title)
            .body(message)
            .appname("conductor")
            .sound_name("default")
            .show()?;
    }

    #[cfg(target_os = "linux")]
    {
        Notification::new()
            .summary(title)
            .body(message)
            .appname("conductor")
            .icon("terminal")
            .timeout(5000)
            .show()?;
    }

    #[cfg(target_os = "windows")]
    {
        Notification::new()
            .summary(title)
            .body(message)
            .appname("conductor")
            .show()?;
    }

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
    if config.enable_sound {
        play_notification_sound(sound);
    }

    if config.enable_desktop_notification {
        if let Err(e) = show_notification("Conductor", message) {
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
    if config.enable_sound {
        play_notification_sound(sound);
    }

    if config.enable_desktop_notification {
        if let Err(e) = show_notification(title, message) {
            debug!("Failed to show desktop notification: {}", e);
        }
    }
}
