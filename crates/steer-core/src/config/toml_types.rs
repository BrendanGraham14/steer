use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiFormat {
    OpenaiResponses,
    OpenaiChat,
    Anthropic,
    Google,
    Xai,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthScheme {
    ApiKey,
    Oauth2,
}

/// Root structure for provider TOML deserialization.
/// Used by both build.rs and runtime code.
#[derive(Debug, Deserialize, Serialize)]
pub struct ProvidersFile {
    pub providers: Vec<ProviderData>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProviderData {
    pub id: String,
    pub name: String,
    pub api_format: ApiFormat,
    pub auth_schemes: Vec<AuthScheme>,
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Root structure for model TOML deserialization.
/// Used by both build.rs and runtime code.
#[derive(Debug, Deserialize, Serialize)]
pub struct ModelsFile {
    pub models: Vec<ModelData>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModelData {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub recommended: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<ModelParameters>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Copy, Default)]
pub struct ThinkingConfig {
    pub enabled: bool,
}

/// Model-specific parameters that can be configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Copy, Default)]
pub struct ModelParameters {
    /// Temperature setting for controlling randomness in generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Maximum number of tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Top-p (nucleus) sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<ThinkingConfig>,
}
