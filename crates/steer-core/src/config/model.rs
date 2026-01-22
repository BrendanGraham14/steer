use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

use super::provider::ProviderId;
use super::toml_types::ModelData;

// Re-export types from toml_types for public use
pub use super::toml_types::{ModelParameters, ThinkingConfig};

/// Identifier for a model (provider + model id string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ModelId {
    pub provider: ProviderId,
    pub id: String,
}

impl ModelId {
    pub fn new(provider: ProviderId, id: impl Into<String>) -> Self {
        Self {
            provider,
            id: id.into(),
        }
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider.storage_key(), self.id)
    }
}

/// Built-in model constants generated from default_catalog.toml
pub mod builtin {

    include!(concat!(env!("OUT_DIR"), "/generated_model_ids.rs"));
}

impl ModelParameters {
    /// Merge two ModelParameters, with `other` taking precedence over `self`.
    /// This allows call_options to override model config defaults.
    pub fn merge(&self, other: &ModelParameters) -> ModelParameters {
        ModelParameters {
            temperature: other.temperature.or(self.temperature),
            max_tokens: other.max_tokens.or(self.max_tokens),
            top_p: other.top_p.or(self.top_p),
            thinking_config: match (self.thinking_config, other.thinking_config) {
                (Some(a), Some(b)) => Some(ThinkingConfig {
                    enabled: b.enabled,
                    effort: b.effort.or(a.effort),
                    budget_tokens: b.budget_tokens.or(a.budget_tokens),
                    include_thoughts: b.include_thoughts.or(a.include_thoughts),
                }),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
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

    /// The model display name. If not provided, the model id is used.
    pub display_name: Option<String>,

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

    /// Merge another ModelConfig into self, with other taking precedence for scalar fields
    /// and arrays being appended uniquely.
    pub fn merge_with(&mut self, other: ModelConfig) {
        // Merge aliases (append unique values)
        for alias in other.aliases {
            if !self.aliases.contains(&alias) {
                self.aliases.push(alias);
            }
        }

        // Override scalar fields (last-write-wins)
        self.recommended = other.recommended;
        if other.display_name.is_some() {
            self.display_name = other.display_name;
        }

        // Merge parameters
        match (&mut self.parameters, other.parameters) {
            (Some(self_params), Some(other_params)) => {
                // Merge parameters
                if let Some(temp) = other_params.temperature {
                    self_params.temperature = Some(temp);
                }
                if let Some(max_tokens) = other_params.max_tokens {
                    self_params.max_tokens = Some(max_tokens);
                }
                if let Some(top_p) = other_params.top_p {
                    self_params.top_p = Some(top_p);
                }
                if let Some(thinking) = other_params.thinking_config {
                    self_params.thinking_config = Some(super::toml_types::ThinkingConfig {
                        enabled: thinking.enabled,
                        effort: thinking
                            .effort
                            .or(self_params.thinking_config.and_then(|t| t.effort)),
                        budget_tokens: thinking
                            .budget_tokens
                            .or(self_params.thinking_config.and_then(|t| t.budget_tokens)),
                        include_thoughts: thinking
                            .include_thoughts
                            .or(self_params.thinking_config.and_then(|t| t.include_thoughts)),
                    });
                }
            }
            (None, Some(other_params)) => {
                self.parameters = Some(other_params);
            }
            _ => {}
        }
    }
}

impl From<ModelData> for ModelConfig {
    fn from(data: ModelData) -> Self {
        ModelConfig {
            provider: ProviderId(data.provider),
            id: data.id,
            display_name: data.display_name,
            aliases: data.aliases,
            recommended: data.recommended,
            parameters: data.parameters,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::provider;

    #[test]
    fn test_model_config_toml_serialization() {
        let config = ModelConfig {
            provider: provider::anthropic(),
            id: "claude-3-opus".to_string(),
            display_name: None,
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

        assert_eq!(config.provider, provider::openai());
        assert_eq!(config.id, "gpt-4");
        assert_eq!(config.display_name, None);
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
            provider: provider::anthropic(),
            id: "claude-3-opus".to_string(),
            display_name: None,
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
