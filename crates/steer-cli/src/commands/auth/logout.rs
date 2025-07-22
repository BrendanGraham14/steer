use clap::Args;
use steer_core::auth::{CredentialType, DefaultAuthStorage};
use eyre::{Result, bail, eyre};
use std::sync::Arc;

#[derive(Args, Clone, Debug)]
pub struct Logout {
    /// Provider to logout from
    pub provider: String,
}

impl Logout {
    pub async fn handle(&self) -> Result<()> {
        match self.provider.as_str() {
            "anthropic" | "claude" => logout_anthropic().await,
            _ => bail!(
                "Unsupported provider: {}. Currently only 'anthropic' is supported.",
                self.provider
            ),
        }
    }
}

async fn logout_anthropic() -> Result<()> {
    println!("Logging out from Anthropic...");

    // Create storage instance
    let storage = Arc::new(
        DefaultAuthStorage::new().map_err(|e| eyre!("Failed to create auth storage: {}", e))?,
    ) as Arc<dyn steer_core::auth::AuthStorage>;

    // Check if any credentials exist
    let has_auth_tokens = storage
        .get_credential("anthropic", CredentialType::AuthTokens)
        .await
        .map_err(|e| eyre!("Failed to check auth tokens: {}", e))?
        .is_some();

    let has_api_key = storage
        .get_credential("anthropic", CredentialType::ApiKey)
        .await
        .map_err(|e| eyre!("Failed to check API key: {}", e))?
        .is_some();

    if !has_auth_tokens && !has_api_key {
        println!("No stored authentication found for Anthropic.");
        return Ok(());
    }

    // Remove auth tokens if they exist
    if has_auth_tokens {
        storage
            .remove_credential("anthropic", CredentialType::AuthTokens)
            .await
            .map_err(|e| eyre!("Failed to remove auth tokens: {}", e))?;
    }

    // Remove API key if it exists
    if has_api_key {
        storage
            .remove_credential("anthropic", CredentialType::ApiKey)
            .await
            .map_err(|e| eyre!("Failed to remove API key: {}", e))?;
    }

    println!("✅ Successfully logged out from Anthropic.");
    println!("You will need to use an API key or login again to use Claude models.");

    Ok(())
}
