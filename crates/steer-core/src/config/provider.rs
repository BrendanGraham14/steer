use serde::{Deserialize, Serialize};
use std::fmt;
use url::Url;

// Re-export enums from toml_types
pub use crate::config::toml_types::{ApiFormat, AuthScheme};

/// Identifier for a provider (built-in or custom).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(transparent)]
pub struct ProviderId(pub String);

impl ProviderId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&'static str> for ProviderId {
    fn from(s: &'static str) -> Self {
        ProviderId(s.to_string())
    }
}

// Generated provider constants
include!(concat!(env!("OUT_DIR"), "/generated_provider_ids.rs"));

impl From<crate::config::toml_types::ProviderData> for ProviderConfig {
    fn from(data: crate::config::toml_types::ProviderData) -> Self {
        ProviderConfig {
            id: ProviderId(data.id),
            name: data.name,
            api_format: data.api_format,
            auth_schemes: data.auth_schemes,
            base_url: data.base_url.and_then(|s| s.parse().ok()),
        }
    }
}

impl ProviderId {
    /// Returns the storage key used for credential persistence.
    /// This provides a single source of truth for storage keys.
    pub fn storage_key(&self) -> String {
        self.0.clone()
    }

    /// Returns the default display name for this provider ID.
    /// Custom providers should override this with their configured name.
    pub fn default_display_name(&self) -> String {
        // Load provider configs to get proper display names
        if let Ok(providers) = builtin_providers() {
            if let Some(config) = providers.iter().find(|p| p.id == *self) {
                return config.name.clone();
            }
        }
        // Fallback: return the ID itself
        self.0.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: ProviderId,
    pub name: String,
    pub api_format: ApiFormat,
    pub auth_schemes: Vec<AuthScheme>,
    /// Optional override for the HTTP base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<Url>,
}

#[derive(Debug, Clone)]
pub struct ProviderModel {
    pub provider: ProviderId,
    pub model_id: String,
    pub name: String,
}

/// Embedded default provider definitions (generated at compile time).
const DEFAULT_PROVIDERS_TOML: &str = include_str!("../../assets/default_providers.toml");

/// Return the list of built-in provider definitions parsed from the embedded TOML.
/// The TOML uses a top-level array of providers (`[[providers]]`).
///
/// Errors are returned as a typed [`crate::error::Error`] to conform to workspace
/// conventions.
pub fn builtin_providers() -> crate::error::Result<Vec<ProviderConfig>> {
    use crate::config::toml_types::ProvidersFile;

    let providers_file: ProvidersFile = toml::from_str(DEFAULT_PROVIDERS_TOML).map_err(|e| {
        crate::error::Error::Configuration(format!(
            "Failed to parse embedded default_providers.toml: {e}"
        ))
    })?;

    let providers = providers_file
        .providers
        .into_iter()
        .map(ProviderConfig::from)
        .collect::<Vec<_>>();

    Ok(providers)
}
