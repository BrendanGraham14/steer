use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tracing::debug;

use crate::config::model::{ModelConfig, ModelId};
use crate::config::provider::ProviderId;
use crate::config::toml_types::Catalog;
use crate::error::Error;

const DEFAULT_CATALOG_TOML: &str = include_str!("../assets/default_catalog.toml");

/// Registry containing all available model configurations.
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    /// Map of ModelId to ModelConfig for fast lookups.
    models: HashMap<ModelId, ModelConfig>,
    /// Map of aliases to ModelIds for alias resolution.
    aliases: HashMap<String, ModelId>,
    /// Set of providers that have at least one model in the registry (for fast provider checks).
    providers: HashSet<ProviderId>,
}

impl ModelRegistry {
    /// Load the model registry with optional additional catalog files.
    ///
    /// Merge order (later overrides earlier):
    /// 1. Built-in defaults from embedded catalog
    /// 2. Discovered catalogs (project, then user)
    /// 3. Additional catalog files specified
    pub fn load(additional_catalogs: &[String]) -> Result<Self, Error> {
        // First, load the built-in models from embedded catalog
        let builtin_catalog: Catalog = toml::from_str(DEFAULT_CATALOG_TOML)
            .map_err(|e| Error::Configuration(format!("Failed to parse default catalog: {e}")))?;

        // Convert TOML models to ModelConfig
        let mut models: Vec<ModelConfig> = builtin_catalog
            .models
            .into_iter()
            .map(ModelConfig::from)
            .collect();

        // Validate providers exist for built-in models
        let mut known_providers: HashMap<ProviderId, bool> = HashMap::new();
        for p in builtin_catalog.providers {
            known_providers.insert(ProviderId(p.id), true);
        }

        // Load discovered catalogs (user + project)
        for path in Self::discover_catalog_paths() {
            if let Some(catalog) = Self::load_catalog_file(&path)? {
                for p in catalog.providers {
                    known_providers.insert(ProviderId(p.id), true);
                }
                let more_models: Vec<ModelConfig> =
                    catalog.models.into_iter().map(ModelConfig::from).collect();
                Self::merge_models(&mut models, more_models);
            }
        }

        // Load additional catalog files
        for catalog_path in additional_catalogs {
            if let Some(catalog) = Self::load_catalog_file(Path::new(catalog_path))? {
                // Add new providers to known set
                for p in catalog.providers {
                    known_providers.insert(ProviderId(p.id), true);
                }

                // Merge models
                let catalog_models: Vec<ModelConfig> =
                    catalog.models.into_iter().map(ModelConfig::from).collect();
                Self::merge_models(&mut models, catalog_models);
            }
        }

        // Validate all models reference known providers
        for model in &models {
            if !known_providers.contains_key(&model.provider) {
                return Err(Error::Configuration(format!(
                    "Model '{}' references unknown provider '{}'",
                    model.id, model.provider
                )));
            }
        }

        // Build the registry from the merged models
        let mut registry = Self {
            models: HashMap::new(),
            aliases: HashMap::new(),
            providers: HashSet::new(),
        };

        for model in models {
            let model_id = ModelId::new(model.provider.clone(), model.id.clone());

            // Track provider presence
            registry.providers.insert(model.provider.clone());

            // Store aliases, ensuring global uniqueness; trim and reject empty aliases
            for raw in &model.aliases {
                let alias = raw.trim();
                if alias.is_empty() {
                    return Err(Error::Configuration(format!(
                        "Empty alias found for {}/{}",
                        model_id.provider.storage_key(),
                        model_id.id.as_str(),
                    )));
                }
                if let Some(existing) = registry.aliases.get(alias) {
                    if existing != &model_id {
                        return Err(Error::Configuration(format!(
                            "Duplicate alias '{}' used by {}/{} and {}/{}",
                            alias,
                            existing.provider.storage_key(),
                            existing.id.as_str(),
                            model_id.provider.storage_key(),
                            model_id.id.as_str(),
                        )));
                    }
                }
                registry.aliases.insert(alias.to_string(), model_id.clone());
            }

            // Store model
            registry.models.insert(model_id, model);
        }

        debug!(
            target: "model_registry::load",
            "Loaded models: {:?}",
            registry.models
        );

        // Validate display_name values: non-empty, unique per provider
        {
            let mut seen: HashMap<ProviderId, HashSet<String>> = HashMap::new();
            for (model_id, cfg) in &registry.models {
                if let Some(name_raw) = cfg.display_name.as_deref() {
                    let name = name_raw.trim();
                    if name.is_empty() {
                        return Err(Error::Configuration(format!(
                            "Invalid display_name '{}' for {}/{}",
                            name_raw,
                            model_id.provider.storage_key(),
                            cfg.id
                        )));
                    }
                    let set = seen.entry(model_id.provider.clone()).or_default();
                    if !set.insert(name.to_string()) {
                        return Err(Error::Configuration(format!(
                            "Duplicate display_name '{}' for provider {}",
                            name,
                            model_id.provider.storage_key()
                        )));
                    }
                }
            }
        }

        // Validate alias collisions across providers (already enforced during build)
        // Add a targeted test to ensure cross-provider duplicate aliases error out.

        Ok(registry)
    }

    /// Build an empty registry (primarily for fallbacks/tests).
    pub fn empty() -> Self {
        Self {
            models: HashMap::new(),
            aliases: HashMap::new(),
            providers: HashSet::new(),
        }
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
    /// - If input contains '/', treats as 'provider/<id|alias>' and resolves accordingly
    ///   Note: model IDs may themselves contain '/', so everything after the first '/'
    ///   is treated as the model ID or alias.
    /// - Otherwise, looks up by alias
    /// - Returns error if not found or invalid
    pub fn resolve(&self, input: &str) -> Result<ModelId, Error> {
        if let Some((provider_str, part_raw)) = input.split_once('/') {
            // Parse provider directly and validate it exists in the registry
            let provider: ProviderId = ProviderId(provider_str.to_string());
            let provider_known = self.providers.contains(&provider);
            if !provider_known {
                return Err(Error::Configuration(format!(
                    "Unknown provider: {provider_str}"
                )));
            }

            let part = part_raw.trim();
            if part.is_empty() {
                return Err(Error::Configuration(
                    "Model name cannot be empty".to_string(),
                ));
            }

            // 1) Try exact model id match (ID can include '/')
            let candidate = ModelId::new(provider.clone(), part.to_string());
            if self.models.contains_key(&candidate) {
                return Ok(candidate);
            }

            // 2) Try alias scoped to the provider
            if let Some(alias_id) = self.aliases.get(part) {
                if alias_id.provider == provider {
                    return Ok(alias_id.clone());
                }
            }

            Err(Error::Configuration(format!(
                "Unknown model or alias: {input}"
            )))
        } else {
            self.by_alias(input)
                .map(|config| ModelId::new(config.provider.clone(), config.id.clone()))
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

    /// Load catalog from a specific path.
    fn load_catalog_file(path: &Path) -> Result<Option<Catalog>, Error> {
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(path).map_err(Error::Io)?;
        // Parse as full catalog only
        let catalog: Catalog = toml::from_str(&content).map_err(|e| {
            Error::Configuration(format!(
                "Failed to parse catalog at {}: {}",
                path.display(),
                e
            ))
        })?;
        Ok(Some(catalog))
    }

    /// Determine default discovery paths for catalogs (user + project)
    fn discover_catalog_paths() -> Vec<PathBuf> {
        // Standardized discovery paths via utils::paths
        let paths: Vec<PathBuf> = crate::utils::paths::AppPaths::discover_catalogs();
        // Do not filter by existence here; load() already checks existence when reading
        paths
    }

    /// Merge user models into the base models file.
    /// Arrays are appended, scalar fields use last-write-wins.
    fn merge_models(base: &mut Vec<ModelConfig>, user_models: Vec<ModelConfig>) {
        // Create a map of existing models by ModelId for efficient lookup
        let mut existing_models: HashMap<ModelId, usize> = HashMap::new();
        for (idx, model) in base.iter().enumerate() {
            existing_models.insert(ModelId::new(model.provider.clone(), model.id.clone()), idx);
        }

        // Process each user model
        for user_model in user_models {
            let key = ModelId::new(user_model.provider.clone(), user_model.id.clone());

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
        // Test that we can parse the built-in catalog
        let catalog: Catalog = toml::from_str(DEFAULT_CATALOG_TOML).unwrap();
        assert!(!catalog.models.is_empty());
        assert!(!catalog.providers.is_empty());

        // Check that we have some expected models
        let has_claude = catalog
            .models
            .iter()
            .any(|m| m.provider == "anthropic" && m.id.contains("claude"));
        assert!(has_claude, "Should have at least one Claude model");
    }

    #[test]
    fn test_registry_creation() {
        // Create a test catalog
        let toml = r#"
[[providers]]
id = "anthropic"
name = "Anthropic"
api_format = "anthropic"
auth_schemes = ["api-key"]

[[models]]
provider = "anthropic"
id = "test-model"
aliases = ["test", "tm"]
recommended = true
parameters = { thinking_config = { enabled = true } }
"#;

        let catalog: Catalog = toml::from_str(toml).unwrap();
        // Convert Catalog to ModelConfig list using From trait
        let models: Vec<ModelConfig> = catalog.models.into_iter().map(ModelConfig::from).collect();

        let mut registry = ModelRegistry {
            models: HashMap::new(),
            aliases: HashMap::new(),
            providers: HashSet::new(),
        };

        for model in models {
            let model_id = ModelId::new(model.provider.clone(), model.id.clone());

            // track provider
            registry.providers.insert(model.provider.clone());

            for alias in &model.aliases {
                registry.aliases.insert(alias.clone(), model_id.clone());
            }

            registry.models.insert(model_id, model);
        }

        // Test get
        let model_id = ModelId::new(provider::anthropic(), "test-model");
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
[[providers]]
id = "anthropic"
name = "Anthropic"
api_format = "anthropic"
auth_schemes = ["api-key"]

[[providers]]
id = "openai"
name = "OpenAI"
api_format = "openai-responses"
auth_schemes = ["api-key"]

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
[[providers]]
id = "google"
name = "Google"
api_format = "google"
auth_schemes = ["api-key"]

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

        let base: Catalog = toml::from_str(base_toml).unwrap();
        let user: Catalog = toml::from_str(user_toml).unwrap();

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
    fn test_load_catalog_from_path() {
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("test_catalog.toml");

        let config = r#"
[[providers]]
id = "anthropic"
name = "Anthropic"
api_format = "anthropic"
auth_schemes = ["api-key"]

[[models]]
provider = "anthropic"
id = "test-model"
aliases = ["test"]
recommended = true
"#;

        fs::write(&config_path, config).unwrap();

        let result = ModelRegistry::load_catalog_file(&config_path).unwrap();
        assert!(result.is_some());

        let catalog = result.unwrap();
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].id, "test-model");
        assert_eq!(catalog.providers.len(), 1);
        assert_eq!(catalog.providers[0].id, "anthropic");
    }

    #[test]
    fn test_resolve_by_provider_and_parts() {
        // Build a small registry manually
        let mut registry = ModelRegistry {
            models: HashMap::new(),
            aliases: HashMap::new(),
            providers: HashSet::new(),
        };
        let prov = provider::anthropic();

        let m1 = ModelConfig {
            provider: prov.clone(),
            id: "id-1".to_string(),
            display_name: Some("NiceName".to_string()),
            aliases: vec!["alias1".into()],
            recommended: false,
            parameters: None,
        };
        let m2 = ModelConfig {
            provider: prov.clone(),
            id: "id-2".to_string(),
            display_name: Some("Other".to_string()),
            aliases: vec!["alias2".into()],
            recommended: false,
            parameters: None,
        };
        let id1 = ModelId::new(prov.clone(), m1.id.clone());
        let id2 = ModelId::new(prov.clone(), m2.id.clone());
        registry.aliases.insert("alias1".into(), id1.clone());
        registry.aliases.insert("alias2".into(), id2.clone());
        registry.models.insert(id1.clone(), m1.clone());
        registry.models.insert(id2.clone(), m2.clone());
        registry.providers.insert(prov.clone());

        // provider/id
        assert_eq!(registry.resolve("anthropic/id-1").unwrap(), id1);
        // provider/display_name should NOT resolve
        assert!(registry.resolve("anthropic/NiceName").is_err());
        // provider/alias should resolve if alias maps to this provider
        assert_eq!(registry.resolve("anthropic/alias2").unwrap(), id2);
        // unknown
        assert!(registry.resolve("anthropic/does-not-exist").is_err());
    }

    #[test]
    fn test_resolve_by_display_name_is_not_supported() {
        // Two models with same display name under same provider
        let mut registry = ModelRegistry {
            models: HashMap::new(),
            aliases: HashMap::new(),
            providers: HashSet::new(),
        };
        let prov = provider::anthropic();
        let m1 = ModelConfig {
            provider: prov.clone(),
            id: "id-1".into(),
            display_name: Some("Same".into()),
            aliases: vec![],
            recommended: false,
            parameters: None,
        };
        let m2 = ModelConfig {
            provider: prov.clone(),
            id: "id-2".into(),
            display_name: Some("Same".into()),
            aliases: vec![],
            recommended: false,
            parameters: None,
        };
        let id1 = ModelId::new(prov.clone(), m1.id.clone());
        let id2 = ModelId::new(prov.clone(), m2.id.clone());
        registry.models.insert(id1, m1);
        registry.models.insert(id2, m2);
        registry.providers.insert(prov.clone());

        // Resolving by display name should not work
        let err = registry.resolve("anthropic/Same").unwrap_err();
        match err {
            Error::Configuration(msg) => assert!(msg.contains("Unknown model or alias")),
            _ => panic!("unexpected error type"),
        }
    }

    #[test]
    fn test_load_rejects_invalid_or_duplicate_display_names() {
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let bad_path = dir.path().join("bad_catalog.toml");
        let dup_path = dir.path().join("dup_catalog.toml");

        // Invalid: empty display_name
        let bad = r#"
[[providers]]
id = "custom"
name = "Custom"
api_format = "openai-responses"
auth_schemes = ["api-key"]

[[models]]
provider = "custom"
id = "m1"
display_name = ""
"#;
        fs::write(&bad_path, bad).unwrap();
        let res = ModelRegistry::load(&[bad_path.to_string_lossy().to_string()]);
        assert!(matches!(res, Err(Error::Configuration(_))));

        // Duplicate display_name within provider
        let dup = r#"
[[providers]]
id = "custom"
name = "Custom"
api_format = "openai-responses"
auth_schemes = ["api-key"]

[[models]]
provider = "custom"
id = "m1"
display_name = "Same"

[[models]]
provider = "custom"
id = "m2"
display_name = "Same"
"#;
        fs::write(&dup_path, dup).unwrap();
        let res2 = ModelRegistry::load(&[dup_path.to_string_lossy().to_string()]);
        assert!(matches!(res2, Err(Error::Configuration(_))));
    }

    #[test]
    fn test_duplicate_aliases_across_providers_error() {
        // Two providers, same alias used by different models => should error on load
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("alias_conflict.toml");
        let toml = r#"
[[providers]]
id = "p1"
name = "P1"
api_format = "openai-responses"
auth_schemes = ["api-key"]

[[providers]]
id = "p2"
name = "P2"
api_format = "openai-responses"
auth_schemes = ["api-key"]

[[models]]
provider = "p1"
id = "m1"
aliases = ["shared"]

[[models]]
provider = "p2"
id = "m2"
aliases = ["shared"]
"#;
        fs::write(&path, toml).unwrap();
        let res = ModelRegistry::load(&[path.to_string_lossy().to_string()]);
        assert!(matches!(res, Err(Error::Configuration(_))));
    }
}
