use crate::config::provider::{ProviderConfig, ProviderId};
use crate::config::toml_types::Catalog;
use std::collections::HashMap;
use std::path::Path;

/// Registry for provider definitions and authentication flow factories.
///
/// This struct is pure domain logic – no networking or gRPC dependencies.
#[derive(Debug, Clone)]
pub struct ProviderRegistry {
    providers: HashMap<ProviderId, ProviderConfig>,
}

const DEFAULT_CATALOG_TOML: &str = include_str!("../../assets/default_catalog.toml");

impl ProviderRegistry {
    /// Load provider definitions with optional additional catalog files.
    ///
    /// Merge order (later overrides earlier):
    /// 1. Built-in defaults from embedded catalog
    /// 2. Additional catalog files specified
    pub fn load(additional_catalogs: &[String]) -> crate::error::Result<Self> {
        let mut providers: HashMap<ProviderId, ProviderConfig> = HashMap::new();

        // 1. Built-in providers from embedded catalog
        let builtin_catalog: Catalog = toml::from_str(DEFAULT_CATALOG_TOML).map_err(|e| {
            crate::error::Error::Configuration(format!(
                "Failed to parse embedded default_catalog.toml: {e}"
            ))
        })?;

        for p in builtin_catalog.providers {
            let config = ProviderConfig::from(p);
            providers.insert(config.id.clone(), config);
        }

        // 2. Additional catalog files
        for catalog_path in additional_catalogs {
            if let Some(catalog) = Self::load_catalog_file(Path::new(catalog_path))? {
                for p in catalog.providers {
                    let config = ProviderConfig::from(p);
                    providers.insert(config.id.clone(), config);
                }
            }
        }

        Ok(Self { providers })
    }

    /// Load a catalog file from disk.
    fn load_catalog_file(path: &Path) -> crate::error::Result<Option<Catalog>> {
        if !path.exists() {
            return Ok(None);
        }

        let contents = std::fs::read_to_string(path)?;
        let catalog: Catalog = toml::from_str(&contents).map_err(|e| {
            crate::error::Error::Configuration(format!("Failed to parse {}: {}", path.display(), e))
        })?;

        Ok(Some(catalog))
    }

    /// Get a provider config by ID.
    pub fn get(&self, id: &ProviderId) -> Option<&ProviderConfig> {
        self.providers.get(id)
    }

    /// Iterate over all provider configs.
    pub fn all(&self) -> impl Iterator<Item = &ProviderConfig> {
        self.providers.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::provider::{self, ApiFormat, AuthScheme, ProviderId};
    use crate::config::toml_types::{Catalog, ProviderData};
    use std::fs;

    // Helper to write a test catalog
    fn write_test_catalog(base_dir: &std::path::Path, catalog: &Catalog) {
        let catalog_path = base_dir.join("test_catalog.toml");
        let toml_str = toml::to_string(catalog).unwrap();
        fs::write(catalog_path, toml_str).unwrap();
    }

    #[test]
    fn loads_builtin_when_no_additional_catalogs() {
        let reg = ProviderRegistry::load(&[]).expect("load registry");
        assert_eq!(reg.all().count(), 4); // Anthropic, OpenAI, Google, xAI
    }

    #[test]
    fn loads_and_merges_additional_catalog() {
        let temp = tempfile::tempdir().unwrap();

        // Create a catalog with override and new provider
        let catalog = Catalog {
            providers: vec![
                ProviderData {
                    id: "anthropic".to_string(),
                    name: "Anthropic (override)".to_string(),
                    api_format: ApiFormat::Anthropic,
                    auth_schemes: vec![AuthScheme::ApiKey],
                    base_url: None,
                },
                ProviderData {
                    id: "myprov".to_string(),
                    name: "My Provider".to_string(),
                    api_format: ApiFormat::OpenaiResponses,
                    auth_schemes: vec![AuthScheme::ApiKey],
                    base_url: None,
                },
            ],
            models: vec![],
        };

        write_test_catalog(temp.path(), &catalog);

        let catalog_path = temp
            .path()
            .join("test_catalog.toml")
            .to_string_lossy()
            .to_string();
        let reg = ProviderRegistry::load(&[catalog_path]).expect("load registry");

        // Overridden provider
        let anthro = reg.get(&provider::anthropic()).unwrap();
        assert_eq!(anthro.name, "Anthropic (override)");

        // Custom provider present
        let custom = reg.get(&ProviderId("myprov".to_string())).unwrap();
        assert_eq!(custom.name, "My Provider");

        assert_eq!(reg.all().count(), 5);
    }
}
