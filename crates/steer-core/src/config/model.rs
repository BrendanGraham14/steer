use serde::{Deserialize, Serialize};

use super::provider::ProviderId;

/// Type alias for model identification as a tuple of (ProviderId, model id string).
pub type ModelId = (ProviderId, String);

/// Model-specific parameters that can be configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}

/// Configuration for a specific model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelConfig {
    /// The provider that offers this model.
    pub provider: ProviderId,

    /// The model identifier (e.g., "gpt-4", "claude-3-opus").
    pub id: String,

    /// Alternative names/aliases for this model.
    #[serde(default)]
    pub aliases: Vec<String>,

    /// Whether this model is recommended for general use.
    #[serde(default)]
    pub recommended: bool,

    /// Whether this model supports thinking/reasoning features.
    #[serde(default)]
    pub supports_thinking: bool,

    /// Optional model-specific parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<ModelParameters>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_config_toml_serialization() {
        let config = ModelConfig {
            provider: ProviderId::Anthropic,
            id: "claude-3-opus".to_string(),
            aliases: vec!["opus".to_string(), "claude-opus".to_string()],
            recommended: true,
            supports_thinking: false,
            parameters: Some(ModelParameters {
                temperature: Some(0.7),
                max_tokens: Some(4096),
                top_p: Some(0.9),
            }),
        };

        // Serialize to TOML
        let toml_string = toml::to_string_pretty(&config).expect("Failed to serialize to TOML");

        // Deserialize back
        let deserialized: ModelConfig =
            toml::from_str(&toml_string).expect("Failed to deserialize from TOML");

        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_model_config_minimal() {
        let toml_str = r#"
            provider = "openai"
            id = "gpt-4"
        "#;

        let config: ModelConfig =
            toml::from_str(toml_str).expect("Failed to deserialize minimal config");

        assert_eq!(config.provider, ProviderId::Openai);
        assert_eq!(config.id, "gpt-4");
        assert_eq!(config.aliases, Vec::<String>::new());
        assert!(!config.recommended);
        assert!(!config.supports_thinking);
        assert!(config.parameters.is_none());
    }

    #[test]
    fn test_model_parameters_partial() {
        let toml_str = r#"
            temperature = 0.5
            max_tokens = 2048
        "#;

        let params: ModelParameters =
            toml::from_str(toml_str).expect("Failed to deserialize parameters");

        assert_eq!(params.temperature, Some(0.5));
        assert_eq!(params.max_tokens, Some(2048));
        assert_eq!(params.top_p, None);
    }
}
