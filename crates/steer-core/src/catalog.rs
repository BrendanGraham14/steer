use crate::auth::ProviderRegistry;
use crate::error::Result;
use crate::model_registry::ModelRegistry;
use std::sync::Arc;

/// Configuration for loading catalog files.
#[derive(Debug, Clone, Default)]
pub struct CatalogConfig {
    /// Additional catalog files to load (absolute or relative paths).
    pub catalog_paths: Vec<String>,
}

impl CatalogConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_catalogs(catalog_paths: Vec<String>) -> Self {
        Self { catalog_paths }
    }
}

/// Load both registries with the same catalog configuration.
pub fn load_registries(
    config: &CatalogConfig,
) -> Result<(Arc<ModelRegistry>, Arc<ProviderRegistry>)> {
    let model_registry = Arc::new(ModelRegistry::load(&config.catalog_paths)?);
    let provider_registry = Arc::new(ProviderRegistry::load(&config.catalog_paths)?);
    Ok((model_registry, provider_registry))
}
