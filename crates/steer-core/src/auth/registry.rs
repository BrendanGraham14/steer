use crate::config::provider::{ProviderConfig, ProviderId, builtin_providers};
use std::collections::HashMap;

/// Registry for provider definitions and authentication flow factories.
///
/// This struct is pure domain logic – no networking or gRPC dependencies.
#[derive(Debug, Clone)]
pub struct ProviderRegistry {
    providers: HashMap<ProviderId, ProviderConfig>,
}

impl ProviderRegistry {
    /// Load provider definitions by merging built-ins with optional user overrides.
    ///
    /// Built-ins come from the embedded `default_providers.toml` file.  Users may
    /// place a `providers.toml` file at `~/.config/conductor/providers.toml` with
    /// the same schema (`[[providers]]` array of tables).  Entries with duplicate
    /// IDs replace the built-ins, and new IDs are appended.
    pub fn load() -> crate::error::Result<Self> {
        let config_dir = dirs::config_dir();
        Self::load_with_config_dir(config_dir.as_deref())
    }

    /// Load provider definitions with an explicit config directory.
    ///
    /// This is primarily for testing. If `config_dir` is None, only built-in
    /// providers are loaded.
    pub fn load_with_config_dir(
        config_dir: Option<&std::path::Path>,
    ) -> crate::error::Result<Self> {
        let mut providers: HashMap<ProviderId, ProviderConfig> = HashMap::new();

        // 1. Built-in providers
        for p in builtin_providers()? {
            providers.insert(p.id.clone(), p);
        }

        // 2. Optional user overrides
        if let Some(cfg_dir) = config_dir {
            let path = cfg_dir.join("conductor").join("providers.toml");
            if path.exists() {
                let contents = std::fs::read_to_string(&path)?;
                #[derive(serde::Deserialize)]
                struct Wrapper {
                    providers: Vec<ProviderConfig>,
                }
                let wrapper: Wrapper = toml::from_str(&contents).map_err(|e| {
                    crate::error::Error::Configuration(format!("Failed to parse {path:?}: {e}"))
                })?;
                for p in wrapper.providers {
                    providers.insert(p.id.clone(), p); // overrides if duplicate
                }
            }
        }

        Ok(Self { providers })
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

    use crate::config::provider::{self, ApiFormat, AuthScheme, ProviderConfig, ProviderId};
    use std::fs;

    // Helper to build a minimal provider config TOML with given definitions
    fn write_user_providers(base_dir: &std::path::Path, providers: &[ProviderConfig]) {
        let steer_dir = base_dir.join("conductor");
        fs::create_dir_all(&steer_dir).unwrap();

        #[derive(serde::Serialize)]
        struct Wrapper<'a> {
            providers: &'a [ProviderConfig],
        }

        let toml_str = toml::to_string(&Wrapper { providers }).unwrap();
        fs::write(steer_dir.join("providers.toml"), toml_str).unwrap();
    }

    #[test]
    fn loads_builtin_when_no_user_file() {
        let reg = ProviderRegistry::load_with_config_dir(None).expect("load registry");
        assert_eq!(reg.all().count(), 4);
    }

    #[test]
    fn loads_builtin_when_config_dir_empty() {
        let temp = tempfile::tempdir().unwrap();
        let reg = ProviderRegistry::load_with_config_dir(Some(temp.path())).expect("load registry");
        assert_eq!(reg.all().count(), 4);
    }

    #[test]
    fn user_overrides_replace_and_extend() {
        let temp = tempfile::tempdir().unwrap();

        // Override Anthropics and add a custom provider
        let override_provider = ProviderConfig {
            id: provider::anthropic(),
            name: "Anthropic (override)".into(),
            api_format: ApiFormat::Anthropic,
            auth_schemes: vec![AuthScheme::ApiKey],
            base_url: None,
        };
        let custom_provider = ProviderConfig {
            id: ProviderId("myprov".to_string()),
            name: "My Provider".into(),
            api_format: ApiFormat::OpenaiResponses,
            auth_schemes: vec![AuthScheme::ApiKey],
            base_url: None,
        };

        write_user_providers(
            temp.path(),
            &[override_provider.clone(), custom_provider.clone()],
        );

        let reg = ProviderRegistry::load_with_config_dir(Some(temp.path())).expect("load registry");

        // Overridden provider
        let anthro = reg.get(&provider::anthropic()).unwrap();
        assert_eq!(anthro.name, "Anthropic (override)");

        // Custom provider present
        let custom = reg.get(&ProviderId("myprov".to_string())).unwrap();
        assert_eq!(custom.name, "My Provider");

        assert_eq!(reg.all().count(), 5);
    }
}
