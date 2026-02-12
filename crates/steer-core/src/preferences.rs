use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use strum::Display;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default, Display)]
#[strum(serialize_all = "kebab-case")]
pub enum EditingMode {
    #[default]
    Simple, // Default - no modal editing
    Vim, // Full vim keybindings
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum NotificationTransport {
    #[default]
    Auto,
    Osc9,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Preferences {
    pub default_model: Option<String>,

    #[serde(default)]
    pub ui: UiPreferences,

    #[serde(default)]
    pub tools: ToolPreferences,

    #[serde(default)]
    pub telemetry: TelemetryPreferences,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiPreferences {
    pub theme: Option<String>,
    #[serde(default)]
    pub notifications: NotificationPreferences,
    pub history_limit: Option<usize>,
    pub provider_priority: Option<Vec<String>>,
    #[serde(default)]
    pub editing_mode: EditingMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPreferences {
    #[serde(default)]
    pub transport: NotificationTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolPreferences {
    pub pre_approved: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryPreferences {
    #[serde(default = "default_telemetry_enabled")]
    pub enabled: bool,
    pub endpoint: Option<String>,
}

fn default_telemetry_enabled() -> bool {
    true
}

impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            transport: NotificationTransport::Auto,
        }
    }
}

impl Default for TelemetryPreferences {
    fn default() -> Self {
        Self {
            enabled: default_telemetry_enabled(),
            endpoint: None,
        }
    }
}

impl Preferences {
    /// Get the path to the preferences file
    pub fn config_path() -> Result<PathBuf, crate::error::Error> {
        let config_dir = dirs::config_dir().ok_or_else(|| {
            crate::error::Error::Configuration("Could not determine config directory".to_string())
        })?;
        Ok(config_dir.join("steer").join("preferences.toml"))
    }

    /// Load preferences from disk, or return defaults if not found
    pub fn load() -> Result<Self, crate::error::Error> {
        let path = Self::config_path()?;

        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            match toml::from_str(&contents) {
                Ok(prefs) => Ok(prefs),
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse preferences file at {:?}: {}. Using defaults.",
                        path,
                        e
                    );
                    Ok(Self::default())
                }
            }
        } else {
            Ok(Self::default())
        }
    }

    /// Save preferences to disk
    pub fn save(&self) -> Result<(), crate::error::Error> {
        let path = Self::config_path()?;

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = toml::to_string_pretty(self).map_err(|e| {
            crate::error::Error::Configuration(format!("Failed to serialize preferences: {e}"))
        })?;

        std::fs::write(&path, contents)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_preferences_default_to_enabled_and_no_endpoint() {
        let telemetry = TelemetryPreferences::default();
        assert!(telemetry.enabled);
        assert_eq!(telemetry.endpoint, None);
    }
}
