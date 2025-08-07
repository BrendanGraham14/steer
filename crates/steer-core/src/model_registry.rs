use std::collections::HashMap;
use std::path::PathBuf;

use tracing::debug;

use crate::config::model::{ModelConfig, ModelId};
use crate::config::provider::ProviderId;
use crate::config::toml_types::ModelsFile as TomlModelsFile;
use crate::error::Error;

const DEFAULT_MODELS_TOML: &str = include_str!("../assets/default_models.toml");

/// Registry containing all available model configurations.
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    /// Map of ModelId to ModelConfig for fast lookups.
    models: HashMap<ModelId, ModelConfig>,
    /// Map of aliases to ModelIds for alias resolution.
    aliases: HashMap<String, ModelId>,
}

impl ModelRegistry {
    /// Load the model registry, merging built-in, user, and project configurations.
    ///
    /// Merge order (later overrides earlier):
    /// 1. Built-in defaults
    /// 2. User-level config
    /// 3. Project-level config
    pub fn load() -> Result<Self, Error> {
        // First, load the built-in models from TOML
        let toml_models: TomlModelsFile = toml::from_str(DEFAULT_MODELS_TOML)
            .map_err(|e| Error::Configuration(format!("Failed to parse default models: {e}")))?;

        // Convert TOML models to ModelConfig
        let mut models: Vec<ModelConfig> = toml_models
            .models
            .into_iter()
            .map(ModelConfig::from)
            .collect();

        // Load user-level config
        if let Some(user_config) = Self::load_user_config()? {
            Self::merge_models(&mut models, user_config);
        }

        // Load project-level config
        if let Some(project_config) = Self::load_project_config()? {
            Self::merge_models(&mut models, project_config);
        }

        // Build the registry from the merged models
        let mut registry = Self {
            models: HashMap::new(),
            aliases: HashMap::new(),
        };

        for model in models {
            let model_id = (model.provider.clone(), model.id.clone());

            // Store aliases
            for alias in &model.aliases {
                registry.aliases.insert(alias.clone(), model_id.clone());
            }

            // Store model
            registry.models.insert(model_id, model);
        }

        debug!(
            target: "model_registry::load",
            "Loaded models: {:?}",
            registry.models
        );

        Ok(registry)
    }

    /// Get a model by its ID.
    pub fn get(&self, id: &ModelId) -> Option<&ModelConfig> {
        self.models.get(id)
    }

    /// Find a model by its alias.
    pub fn by_alias(&self, alias: &str) -> Option<&ModelConfig> {
        self.aliases.get(alias).and_then(|id| self.models.get(id))
    }
    /// Resolve a model string to a ModelId.
    /// - If input contains '/', treats as 'provider/id' and parses accordingly
    /// - Otherwise, looks up by alias
    /// - Returns error if not found or invalid
    pub fn resolve(&self, input: &str) -> Result<ModelId, Error> {
        if let Some((provider_str, id)) = input.split_once('/') {
            // Try to deserialize the provider string using serde
            let provider: ProviderId =
                serde_json::from_value(serde_json::Value::String(provider_str.to_string()))
                    .map_err(|_| {
                        Error::Configuration(format!("Invalid provider: {provider_str}"))
                    })?;
            Ok((provider, id.to_string()))
        } else {
            self.by_alias(input)
                .map(|config| (config.provider.clone(), config.id.clone()))
                .ok_or_else(|| Error::Configuration(format!("Unknown model or alias: {input}")))
        }
    }

    pub fn recommended(&self) -> impl Iterator<Item = &ModelConfig> {
        self.models.values().filter(|model| model.recommended)
    }

    /// Get all models in the registry
    pub fn all(&self) -> impl Iterator<Item = &ModelConfig> {
        self.models.values()
    }

    /// Load user configuration from the standard location.
    fn load_user_config() -> Result<Option<Vec<ModelConfig>>, Error> {
        let config_path = Self::get_user_config_path()?;
        Self::load_config_from_path(config_path)
    }

    /// Load project configuration from the current workspace.
    fn load_project_config() -> Result<Option<Vec<ModelConfig>>, Error> {
        let config_path = PathBuf::from("models.toml");
        Self::load_config_from_path(config_path)
    }

    /// Load configuration from a specific path.
    fn load_config_from_path(path: PathBuf) -> Result<Option<Vec<ModelConfig>>, Error> {
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&path).map_err(Error::Io)?;

        let toml_models: TomlModelsFile = toml::from_str(&content).map_err(|e| {
            Error::Configuration(format!(
                "Failed to parse models at {}: {}",
                path.display(),
                e
            ))
        })?;

        // Convert TOML models to ModelConfig
        let models = toml_models
            .models
            .into_iter()
            .map(ModelConfig::from)
            .collect();

        Ok(Some(models))
    }

    /// Get the path to the user's models configuration file.
    fn get_user_config_path() -> Result<PathBuf, Error> {
        use directories::ProjectDirs;

        let proj_dirs = ProjectDirs::from("", "", "conductor").ok_or_else(|| {
            Error::Configuration("Cannot determine project directories".to_string())
        })?;

        Ok(proj_dirs.config_dir().join("models.toml"))
    }

    /// Merge user models into the base models file.
    /// Arrays are appended, scalar fields use last-write-wins.
    fn merge_models(base: &mut Vec<ModelConfig>, user_models: Vec<ModelConfig>) {
        // Create a map of existing models by (provider, id) for efficient lookup
        let mut existing_models: HashMap<(ProviderId, String), usize> = HashMap::new();
        for (idx, model) in base.iter().enumerate() {
            existing_models.insert((model.provider.clone(), model.id.clone()), idx);
        }

        // Process each user model
        for user_model in user_models {
            let key = (user_model.provider.clone(), user_model.id.clone());

            if let Some(&idx) = existing_models.get(&key) {
                // Model exists - merge it
                base[idx].merge_with(user_model);
            } else {
                // New model - add it
                base.push(user_model);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::provider;

    #[test]
    fn test_load_builtin_models() {
        // Test that we can parse the built-in models
        let models_file: TomlModelsFile = toml::from_str(DEFAULT_MODELS_TOML).unwrap();
        assert!(!models_file.models.is_empty());

        // Check that we have some expected models
        let has_claude = models_file
            .models
            .iter()
            .any(|m| m.provider == "anthropic" && m.id.contains("claude"));
        assert!(has_claude, "Should have at least one Claude model");
    }

    #[test]
    fn test_registry_creation() {
        // Create a test models file
        let toml = r#"
[[models]]
provider = "anthropic"
id = "test-model"
aliases = ["test", "tm"]
recommended = true
parameters = { thinking_config = { enabled = true } }
"#;

        let models_file: TomlModelsFile = toml::from_str(toml).unwrap();
        // Convert TomlModelsFile to ModelConfig list using From trait
        let models: Vec<ModelConfig> = models_file
            .models
            .into_iter()
            .map(ModelConfig::from)
            .collect();

        let mut registry = ModelRegistry {
            models: HashMap::new(),
            aliases: HashMap::new(),
        };

        for model in models {
            let model_id = (model.provider.clone(), model.id.clone());

            for alias in &model.aliases {
                registry.aliases.insert(alias.clone(), model_id.clone());
            }

            registry.models.insert(model_id, model);
        }

        // Test get
        let model_id = (provider::anthropic(), "test-model".to_string());
        let model = registry.get(&model_id).unwrap();
        assert_eq!(model.id, "test-model");
        assert!(model.recommended);

        // Test parameters were parsed correctly
        assert!(model.parameters.is_some());
        let params = model.parameters.unwrap();
        assert!(params.thinking_config.is_some());
        assert!(params.thinking_config.unwrap().enabled);

        // Test by_alias
        let model_by_alias = registry.by_alias("test").unwrap();
        assert_eq!(model_by_alias.id, "test-model");

        let model_by_alias2 = registry.by_alias("tm").unwrap();
        assert_eq!(model_by_alias2.id, "test-model");

        // Test recommended
        let recommended: Vec<_> = registry.recommended().collect();
        assert_eq!(recommended.len(), 1);
        assert_eq!(recommended[0].id, "test-model");
    }

    #[test]
    fn test_merge_models() {
        let base_toml = r#"
[[models]]
provider = "anthropic"
id = "claude-3"
aliases = ["claude"]
recommended = false
parameters = { temperature = 0.7, max_tokens = 2048 }

[[models]]
provider = "openai"
id = "gpt-4"
aliases = ["gpt"]
recommended = true
"#;

        let user_toml = r#"
[[models]]
provider = "anthropic"
id = "claude-3"
aliases = ["c3", "claude3"]
recommended = true
parameters = { temperature = 0.9, thinking_config = { enabled = true } }

[[models]]
provider = "google"
id = "gemini-pro"
aliases = ["gemini"]
recommended = true
parameters = { temperature = 0.5, top_p = 0.95 }
"#;

        let base: TomlModelsFile = toml::from_str(base_toml).unwrap();
        let user: TomlModelsFile = toml::from_str(user_toml).unwrap();

        // Convert to ModelConfig using From trait
        let base_models: Vec<_> = base.models.into_iter().map(ModelConfig::from).collect();
        let user_models: Vec<_> = user.models.into_iter().map(ModelConfig::from).collect();

        let mut base_models_mut = base_models;
        ModelRegistry::merge_models(&mut base_models_mut, user_models);

        // Check that we have 3 models total
        assert_eq!(base_models_mut.len(), 3);

        // Check the merged Claude model
        let claude = base_models_mut
            .iter()
            .find(|m| m.provider == provider::anthropic() && m.id == "claude-3")
            .unwrap();

        // Aliases should be merged
        assert_eq!(claude.aliases.len(), 3);
        assert!(claude.aliases.contains(&"claude".to_string()));
        assert!(claude.aliases.contains(&"c3".to_string()));
        assert!(claude.aliases.contains(&"claude3".to_string()));

        // Scalar fields should be overridden
        assert!(claude.recommended);

        // Parameters should be merged (user overrides base)
        assert!(claude.parameters.is_some());
        let claude_params = claude.parameters.unwrap();
        assert_eq!(claude_params.temperature, Some(0.9)); // overridden from 0.7
        assert_eq!(claude_params.max_tokens, Some(2048)); // kept from base
        assert!(claude_params.thinking_config.is_some());
        assert!(claude_params.thinking_config.unwrap().enabled);

        // Check that GPT-4 is unchanged
        let gpt4 = base_models_mut
            .iter()
            .find(|m| m.provider == provider::openai() && m.id == "gpt-4")
            .unwrap();
        assert!(gpt4.recommended);
        assert!(gpt4.parameters.is_none()); // No parameters in either base or user

        // Check that new model was added
        let gemini = base_models_mut
            .iter()
            .find(|m| m.provider == provider::google() && m.id == "gemini-pro")
            .unwrap();
        assert!(gemini.recommended);
        assert!(gemini.parameters.is_some());
        let gemini_params = gemini.parameters.unwrap();
        assert_eq!(gemini_params.temperature, Some(0.5));
        assert_eq!(gemini_params.top_p, Some(0.95));
    }

    #[test]
    fn test_load_config_from_path() {
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("test_models.toml");

        let config = r#"
[[models]]
provider = "anthropic"
id = "test-model"
aliases = ["test"]
recommended = true
"#;

        fs::write(&config_path, config).unwrap();

        let result = ModelRegistry::load_config_from_path(config_path).unwrap();
        assert!(result.is_some());

        let models_file = result.unwrap();
        assert_eq!(models_file.len(), 1);
        assert_eq!(models_file[0].id, "test-model");
    }
}
