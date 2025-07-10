use crate::api::ProviderKind;
use crate::auth::{AuthStorage, Credential};
use crate::error::{Error, Result};

/// Import an API key for a provider
pub async fn import_api_key(
    provider: ProviderKind,
    api_key: String,
    storage: &dyn AuthStorage,
) -> Result<()> {
    if api_key.is_empty() {
        return Err(Error::Configuration("API key cannot be empty".to_string()));
    }

    storage
        .set_credential(&provider.to_string(), Credential::ApiKey { value: api_key })
        .await
        .map_err(|e| Error::Configuration(format!("Failed to store API key: {e}")))?;

    Ok(())
}
