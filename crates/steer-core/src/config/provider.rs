use serde::{Deserialize, Serialize};
use url::Url;

/// Identifier for a provider (built-in or custom).
///
/// The built-ins are kept in snake_case to match user-facing identifiers and TOML defaults.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    Openai,
    Gemini,
    Xai,
    /// Arbitrary, user-supplied provider identifier.
    Custom(String),
}

pub type ProviderId = Provider;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    Openai,
    Anthropic,
    Gemini,
    Xai,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    ApiKey,
    Oauth2,
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
/// The TOML uses a top-level array of tables (`[[providers]]`).
///
/// Errors are returned as a typed [`crate::error::Error`] to conform to workspace
/// conventions.
pub fn builtin_providers() -> crate::error::Result<Vec<ProviderConfig>> {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        providers: Vec<ProviderConfig>,
    }

    let wrapper: Wrapper = toml::from_str(DEFAULT_PROVIDERS_TOML).map_err(|e| {
        crate::error::Error::Configuration(format!(
            "Failed to parse embedded default_providers.toml: {e}"
        ))
    })?;
    Ok(wrapper.providers)
}
