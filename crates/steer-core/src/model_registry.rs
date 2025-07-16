use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::model::{ModelConfig, ModelId};
use crate::config::provider::ProviderId;
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

/// Root structure for TOML deserialization.
#[derive(Debug, Deserialize, Serialize)]
struct ModelsFile {
    models: Vec<ModelConfig>,
}

impl ModelRegistry {
    /// Load the model registry, merging built-in, user, and project configurations.
    ///
    /// Merge order (later overrides earlier):
    /// 1. Built-in defaults
    /// 2. User-level config
    /// 3. Project-level config
    pub fn load() -> Result<Self, Error> {
        // First, load the built-in models
        let mut models_file: ModelsFile = toml::from_str(DEFAULT_MODELS_TOML)
            .map_err(|e| Error::Configuration(format!("Failed to parse default models: {e}")))?;

        // Load user-level config
        if let Some(user_config) = Self::load_user_config()? {
            Self::merge_models(&mut models_file, user_config);
        }

        // Load project-level config
        if let Some(project_config) = Self::load_project_config()? {
            Self::merge_models(&mut models_file, project_config);
        }

        // Build the registry from the merged models
        let mut registry = Self {
            models: HashMap::new(),
            aliases: HashMap::new(),
        };

        for model in models_file.models {
            let model_id = (model.provider.clone(), model.id.clone());

            // Store aliases
            for alias in &model.aliases {
                registry.aliases.insert(alias.clone(), model_id.clone());
            }

            // Store model
            registry.models.insert(model_id, model);
        }

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

    /// Get an iterator over all recommended models.
    pub fn recommended(&self) -> impl Iterator<Item = &ModelConfig> {
        self.models.values().filter(|model| model.recommended)
    }

    /// Load user configuration from the standard location.
    fn load_user_config() -> Result<Option<ModelsFile>, Error> {
        let config_path = Self::get_user_config_path()?;
        Self::load_config_from_path(config_path)
    }

    /// Load project configuration from the current workspace.
    fn load_project_config() -> Result<Option<ModelsFile>, Error> {
        let config_path = PathBuf::from("models.toml");
        Self::load_config_from_path(config_path)
    }

    /// Load configuration from a specific path.
    fn load_config_from_path(path: PathBuf) -> Result<Option<ModelsFile>, Error> {
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&path).map_err(Error::Io)?;

        let models = toml::from_str(&content).map_err(|e| {
            Error::Configuration(format!(
                "Failed to parse models at {}: {}",
                path.display(),
                e
            ))
        })?;

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
    fn merge_models(base: &mut ModelsFile, user: ModelsFile) {
        // Create a map of existing models by (provider, id) for efficient lookup
        let mut existing_models: HashMap<(ProviderId, String), usize> = HashMap::new();
        for (idx, model) in base.models.iter().enumerate() {
            existing_models.insert((model.provider.clone(), model.id.clone()), idx);
        }

        // Process each user model
        for user_model in user.models {
            let key = (user_model.provider.clone(), user_model.id.clone());

            if let Some(&idx) = existing_models.get(&key) {
                // Model exists - merge it (last-write-wins for scalars, append for arrays)
                let base_model = &mut base.models[idx];

                // Merge aliases (append unique values)
                for alias in user_model.aliases {
                    if !base_model.aliases.contains(&alias) {
                        base_model.aliases.push(alias);
                    }
                }

                // Override scalar fields (last-write-wins)
                base_model.recommended = user_model.recommended;
                base_model.supports_thinking = user_model.supports_thinking;

                // Override parameters if provided
                if user_model.parameters.is_some() {
                    base_model.parameters = user_model.parameters;
                }
            } else {
                // New model - add it
                base.models.push(user_model);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_builtin_models() {
        // Test that we can parse the built-in models
        let models_file: ModelsFile = toml::from_str(DEFAULT_MODELS_TOML).unwrap();
        assert!(!models_file.models.is_empty());

        // Check that we have some expected models
        let has_claude = models_file
            .models
            .iter()
            .any(|m| m.provider == ProviderId::Anthropic && m.id.contains("claude"));
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
supports_thinking = false

[models.parameters]
temperature = 0.7
max_tokens = 1000
"#;

        let models_file: ModelsFile = toml::from_str(toml).unwrap();
        let mut registry = ModelRegistry {
            models: HashMap::new(),
            aliases: HashMap::new(),
        };

        for model in models_file.models {
            let model_id = (model.provider.clone(), model.id.clone());

            for alias in &model.aliases {
                registry.aliases.insert(alias.clone(), model_id.clone());
            }

            registry.models.insert(model_id, model);
        }

        // Test get
        let model_id = (ProviderId::Anthropic, "test-model".to_string());
        let model = registry.get(&model_id).unwrap();
        assert_eq!(model.id, "test-model");
        assert!(model.recommended);

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
supports_thinking = false

[[models]]
provider = "openai"
id = "gpt-4"
aliases = ["gpt"]
recommended = true
supports_thinking = false
"#;

        let user_toml = r#"
[[models]]
provider = "anthropic"
id = "claude-3"
aliases = ["c3", "claude3"]
recommended = true
supports_thinking = true

[models.parameters]
temperature = 0.8

[[models]]
provider = "gemini"
id = "gemini-pro"
aliases = ["gemini"]
recommended = true
supports_thinking = false
"#;

        let mut base: ModelsFile = toml::from_str(base_toml).unwrap();
        let user: ModelsFile = toml::from_str(user_toml).unwrap();

        ModelRegistry::merge_models(&mut base, user);

        // Check that we have 3 models total
        assert_eq!(base.models.len(), 3);

        // Check the merged Claude model
        let claude = base
            .models
            .iter()
            .find(|m| m.provider == ProviderId::Anthropic && m.id == "claude-3")
            .unwrap();

        // Aliases should be merged
        assert_eq!(claude.aliases.len(), 3);
        assert!(claude.aliases.contains(&"claude".to_string()));
        assert!(claude.aliases.contains(&"c3".to_string()));
        assert!(claude.aliases.contains(&"claude3".to_string()));

        // Scalar fields should be overridden
        assert!(claude.recommended);
        assert!(claude.supports_thinking);

        // Parameters should be set
        assert!(claude.parameters.is_some());
        let params = claude.parameters.as_ref().unwrap();
        assert_eq!(params.temperature, Some(0.8));

        // Check that GPT-4 is unchanged
        let gpt4 = base
            .models
            .iter()
            .find(|m| m.provider == ProviderId::Openai && m.id == "gpt-4")
            .unwrap();
        assert!(gpt4.recommended);
        assert!(!gpt4.supports_thinking);

        // Check that Gemini was added
        let gemini = base
            .models
            .iter()
            .find(|m| m.provider == ProviderId::Gemini && m.id == "gemini-pro")
            .unwrap();
        assert!(gemini.recommended);
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
supports_thinking = false
"#;

        fs::write(&config_path, config).unwrap();

        let result = ModelRegistry::load_config_from_path(config_path).unwrap();
        assert!(result.is_some());

        let models_file = result.unwrap();
        assert_eq!(models_file.models.len(), 1);
        assert_eq!(models_file.models[0].id, "test-model");
    }
}
