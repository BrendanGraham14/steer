use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum EditingMode {
    #[default]
    Simple, // Default - no modal editing
    Vim, // Full vim keybindings
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Preferences {
    #[serde(default)]
    pub ui: UiPreferences,

    #[serde(default)]
    pub keybindings: KeyBindings,

    #[serde(default)]
    pub tools: ToolPreferences,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiPreferences {
    pub default_model: Option<String>,
    pub theme: Option<String>,
    pub notifications: NotificationPreferences,
    pub history_limit: Option<usize>,
    pub provider_priority: Option<Vec<String>>,
    #[serde(default)]
    pub editing_mode: EditingMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPreferences {
    pub sound: bool,
    pub desktop: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyBindings {
    pub cancel: Option<String>,
    pub model_selection: Option<String>,
    pub clear: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolPreferences {
    pub pre_approved: Vec<String>,
}

impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            sound: true,
            desktop: true,
        }
    }
}

impl Preferences {
    /// Get the path to the preferences file
    pub fn config_path() -> Result<PathBuf, crate::error::Error> {
        let config_dir = dirs::config_dir().ok_or_else(|| {
            crate::error::Error::Configuration("Could not determine config directory".to_string())
        })?;
        Ok(config_dir.join("conductor").join("preferences.toml"))
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
