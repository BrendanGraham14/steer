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

#[derive(Debug, Deserialize, Serialize)]
pub struct ProviderData {
    pub id: String,
    pub name: String,
    pub api_format: ApiFormat,
    pub auth_schemes: Vec<AuthScheme>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModelData {
    pub provider: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub recommended: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<ModelParameters>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingEffort {
    Low,
    Medium,
    High,
    XHigh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Copy, Default)]
pub struct ThinkingConfig {
    /// Enables provider-specific reasoning/thinking features.
    pub enabled: bool,
    /// Effort level for providers that support qualitative control (e.g., OpenAI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<ThinkingEffort>,
    /// Token budget for providers that support quantitative limits (e.g., Anthropic, Google).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    /// Include model thoughts in the visible output when supported (e.g., Gemini).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
}

/// Model-specific parameters that can be configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Copy, Default)]
pub struct ModelParameters {
    /// Temperature setting for controlling randomness in generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Maximum number of output tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,

    /// Top-p (nucleus) sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<ThinkingConfig>,
}

/// Unified catalog structure containing both providers and models.
/// Both arrays are optional to allow partial catalogs.
#[derive(Debug, Deserialize, Serialize)]
pub struct Catalog {
    #[serde(default)]
    pub providers: Vec<ProviderData>,
    #[serde(default)]
    pub models: Vec<ModelData>,
}
