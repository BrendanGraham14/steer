use serde::{Deserialize, Serialize};

use super::provider::ProviderId;

/// Type alias for model identification as a tuple of (ProviderId, model id string).
pub type ModelId = (ProviderId, String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Copy)]
pub struct ThinkingConfig {
    pub enabled: bool,
}

/// Model-specific parameters that can be configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Copy)]
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

impl ModelParameters {
    /// Merge two ModelParameters, with `other` taking precedence over `self`.
    /// This allows call_options to override model config defaults.
    pub fn merge(&self, other: &ModelParameters) -> ModelParameters {
        ModelParameters {
            temperature: other.temperature.or(self.temperature),
            max_tokens: other.max_tokens.or(self.max_tokens),
            top_p: other.top_p.or(self.top_p),
            thinking_config: other.thinking_config.or(self.thinking_config),
        }
    }
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

    /// Optional model-specific parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<ModelParameters>,
}

impl ModelConfig {
    /// Get the effective parameters by merging model config with call options.
    /// Call options take precedence over model config defaults.
    pub fn effective_parameters(
        &self,
        call_options: Option<&ModelParameters>,
    ) -> Option<ModelParameters> {
        match (&self.parameters, call_options) {
            (Some(config_params), Some(call_params)) => Some(config_params.merge(call_params)),
            (Some(config_params), None) => Some(*config_params),
            (None, Some(call_params)) => Some(*call_params),
            (None, None) => None,
        }
    }
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
            parameters: Some(ModelParameters {
                temperature: Some(0.7),
                max_tokens: Some(4096),
                top_p: Some(0.9),
                thinking_config: None,
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

    #[test]
    fn test_model_parameters_merge() {
        let base = ModelParameters {
            temperature: Some(0.7),
            max_tokens: Some(1000),
            top_p: Some(0.9),
            thinking_config: None,
        };

        let override_params = ModelParameters {
            temperature: Some(0.5),
            max_tokens: None,
            top_p: Some(0.95),
            thinking_config: None,
        };

        let merged = base.merge(&override_params);
        assert_eq!(merged.temperature, Some(0.5)); // overridden
        assert_eq!(merged.max_tokens, Some(1000)); // kept from base
        assert_eq!(merged.top_p, Some(0.95)); // overridden
    }

    #[test]
    fn test_model_config_effective_parameters() {
        let config = ModelConfig {
            provider: ProviderId::Anthropic,
            id: "claude-3-opus".to_string(),
            aliases: vec![],
            recommended: true,
            parameters: Some(ModelParameters {
                temperature: Some(0.7),
                max_tokens: Some(4096),
                top_p: None,
                thinking_config: None,
            }),
        };

        // Test with no call options
        let effective = config.effective_parameters(None).unwrap();
        assert_eq!(effective.temperature, Some(0.7));
        assert_eq!(effective.max_tokens, Some(4096));
        assert_eq!(effective.top_p, None);

        // Test with call options
        let call_options = ModelParameters {
            temperature: Some(0.9),
            max_tokens: None,
            top_p: Some(0.95),
            thinking_config: None,
        };
        let effective = config.effective_parameters(Some(&call_options)).unwrap();
        assert_eq!(effective.temperature, Some(0.9)); // overridden
        assert_eq!(effective.max_tokens, Some(4096)); // kept from config
        assert_eq!(effective.top_p, Some(0.95)); // added
    }
}
