use conductor_core::auth::DefaultAuthStorage;
use eyre::{Result, bail, eyre};
use std::sync::Arc;

pub async fn execute(provider: &str) -> Result<()> {
    match provider {
        "anthropic" | "claude" => logout_anthropic().await,
        _ => bail!(
            "Unsupported provider: {}. Currently only 'anthropic' is supported.",
            provider
        ),
    }
}

async fn logout_anthropic() -> Result<()> {
    println!("Logging out from Anthropic...");

    // Create storage instance
    let storage = Arc::new(
        DefaultAuthStorage::new().map_err(|e| eyre!("Failed to create auth storage: {}", e))?,
    ) as Arc<dyn conductor_core::auth::AuthStorage>;

    // Check if tokens exist
    let has_tokens = storage
        .get_tokens("anthropic")
        .await
        .map_err(|e| eyre!("Failed to check tokens: {}", e))?
        .is_some();

    if !has_tokens {
        println!("No stored authentication found for Anthropic.");
        return Ok(());
    }

    // Remove tokens
    storage
        .remove_tokens("anthropic")
        .await
        .map_err(|e| eyre!("Failed to remove tokens: {}", e))?;

    println!("✅ Successfully logged out from Anthropic.");
    println!("You will need to use an API key or login again to use Claude models.");

    Ok(())
}
