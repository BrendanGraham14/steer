use crate::auth::{AuthPluginRegistry, ProviderRegistry};
use crate::catalog::CatalogConfig;
use crate::config::LlmConfigProvider;
use crate::error::Result;
use crate::model_registry::ModelRegistry;
use std::sync::Arc;

pub mod conversation;
pub mod domain;
pub mod system_context;
pub mod validation;

pub use conversation::{Message, MessageData, MessageGraph};
pub use steer_workspace::EnvironmentInfo;
pub use system_context::SystemContext;

#[derive(Clone)]
pub struct AppConfig {
    pub llm_config_provider: LlmConfigProvider,
    pub model_registry: Arc<ModelRegistry>,
    pub provider_registry: Arc<ProviderRegistry>,
}

impl AppConfig {
    pub fn from_auth_storage(auth_storage: Arc<dyn crate::auth::AuthStorage>) -> Result<Self> {
        Self::from_auth_storage_with_catalog(auth_storage, CatalogConfig::default())
    }

    pub fn from_auth_storage_with_catalog(
        auth_storage: Arc<dyn crate::auth::AuthStorage>,
        catalog_config: CatalogConfig,
    ) -> Result<Self> {
        let llm_config_provider = LlmConfigProvider::new(auth_storage)?;
        let model_registry = Arc::new(ModelRegistry::load(&catalog_config.catalog_paths)?);
        let provider_registry = Arc::new(ProviderRegistry::load(&catalog_config.catalog_paths)?);

        Ok(Self {
            llm_config_provider,
            model_registry,
            provider_registry,
        })
    }

    #[cfg(not(test))]
    pub fn new() -> Result<Self> {
        let auth_storage = Arc::new(crate::auth::DefaultAuthStorage::new()?);
        Self::from_auth_storage(auth_storage)
    }
}

impl Default for AppConfig {
    // Test default config cannot fail with in-memory storage
    fn default() -> Self {
        let storage = Arc::new(crate::test_utils::InMemoryAuthStorage::new());
        Self::from_auth_storage(storage.clone()).unwrap_or_else(|error| {
            let catalog_config = crate::catalog::CatalogConfig::default();
            let (model_registry, provider_registry) =
                crate::catalog::load_registries(&catalog_config).unwrap_or_else(|fallback_error| {
                    tracing::error!(
                        error = %fallback_error,
                        "AppConfig::default failed to load registries; using empty defaults"
                    );
                    (
                    Arc::new(
                        crate::model_registry::ModelRegistry::load(&[]).unwrap_or_else(
                            |fallback_error| {
                                tracing::error!(
                                    error = %fallback_error,
                                    "Failed to load built-in model registry; using empty registry"
                                );
                                crate::model_registry::ModelRegistry::empty()
                            },
                        ),
                    ),
                    Arc::new(
                        ProviderRegistry::load(&[]).unwrap_or_else(|fallback_error| {
                            tracing::error!(
                                error = %fallback_error,
                                "Failed to load built-in provider registry; using empty registry"
                            );
                            ProviderRegistry::empty()
                        }),
                    ),
                )
                });
            let llm_config_provider = match LlmConfigProvider::new(storage) {
                Ok(provider) => provider,
                Err(fallback_error) => {
                    tracing::error!(
                        error = %fallback_error,
                        "Failed to initialize LLM config provider; using fallback provider"
                    );
                    LlmConfigProvider::new_with_plugins(
                        Arc::new(crate::test_utils::InMemoryAuthStorage::new()),
                        Arc::new(AuthPluginRegistry::new()),
                    )
                }
            };

            tracing::error!(
                error = %error,
                "AppConfig::default failed to load; using fallback defaults"
            );

            Self {
                llm_config_provider,
                model_registry,
                provider_registry,
            }
        })
    }
}
