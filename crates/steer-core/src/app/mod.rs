use crate::auth::ProviderRegistry;
use crate::catalog::CatalogConfig;
use crate::config::LlmConfigProvider;
use crate::error::Result;
use crate::model_registry::ModelRegistry;
use std::sync::Arc;

pub mod conversation;
pub mod domain;
pub mod validation;

pub use conversation::{Message, MessageData, MessageGraph};
pub use steer_workspace::EnvironmentInfo;

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
    fn default() -> Self {
        let storage = Arc::new(crate::test_utils::InMemoryAuthStorage::new());
        Self::from_auth_storage(storage).expect("Failed to create test AppConfig")
    }
}
